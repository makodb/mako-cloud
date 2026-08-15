import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

import { assertNoSecrets, assertValidPlan } from "../public-beta-plan-lib.js";
import { buildPlan, PlanBlockedError } from "../public-beta-planner.js";

const root = resolve(import.meta.dirname, "../../..");
const request = await readJson(resolve(root, "infra/proxmox/public-beta/request.json"));
const baseline = await readJson(resolve(import.meta.dirname, "fixtures/public-beta-success.json"));

test("a complete unambiguous fixture produces a hash-bound create plan", () => {
  const plan = build(baseline);
  assert.equal(plan.checks.conflictFree, true);
  assert.equal(plan.changes.classification, "create");
  assert.equal(plan.network.bridge, "vmbr0");
  assertValidPlan(plan);
});

test("multiple matching bridges and gateways are rejected", () => {
  const fixture = clone(baseline);
  fixture.discovery.networks.push({
    ...fixture.discovery.networks[0],
    bridge: "vmbr1",
    gateway: "130.245.173.254",
  });
  assertBlocked(fixture, "expected exactly one active bridge");
});

test("insufficient host memory remains a hard capacity blocker", () => {
  const fixture = clone(baseline);
  fixture.discovery.selectedNode.memoryBytes = 20 * 1024 ** 3;
  fixture.discovery.selectedNode.memoryUsedBytes = 8 * 1024 ** 3;
  assertBlocked(fixture, "host memory");
});

test("an already-used proposed VM identifier is rejected", () => {
  const fixture = clone(baseline);
  fixture.discovery.guests.push(guest({ vmId: fixture.discovery.nextVmId, name: "other" }));
  assertBlocked(fixture, "VM identifier");
});

test("a DNS mismatch is rejected", () => {
  const fixture = clone(baseline);
  fixture.conflictEvidence.dns.publicAnswers[0].a = ["192.0.2.20"];
  assertBlocked(fixture, "resolves");
});

test("an active address owner without the expected guest identity is rejected", () => {
  const fixture = clone(baseline);
  fixture.conflictEvidence.address = {
    ...fixture.conflictEvidence.address,
    neighborState: "reachable",
    neighborMac: "aa:bb:cc:dd:ee:ff",
    activeOwnerDetected: true,
  };
  assertBlocked(fixture, "unrecognized active owner");
});

test("ambiguous storage is rejected instead of guessed", () => {
  const fixture = clone(baseline);
  fixture.discovery.storage.push({
    ...fixture.discovery.storage[0],
    id: "another-zfs",
  });
  assertBlocked(fixture, "OS storage is ambiguous");
});

test("secret-bearing fields cannot enter discovery or plan evidence", () => {
  assert.throws(
    () => assertNoSecrets({ nested: { serviceToken: "must-not-appear" } }),
    /forbidden secret-bearing field/,
  );
});

test("changing a hash-bound plan is detected as plan drift", () => {
  const plan = build(baseline);
  plan.resources.memoryMiB += 1024;
  assert.throws(() => assertValidPlan(plan), /plan hash does not match/);
});

test("a matching existing guest can be planned idempotently", () => {
  const fixture = clone(baseline);
  const expected = guest({
    vmId: fixture.discovery.nextVmId,
    name: request.identity.vmName,
    address: request.identity.ipv4Address,
    mac: "02:00:00:00:00:01",
  });
  fixture.discovery.guests.push(expected);
  fixture.conflictEvidence.address = {
    ...fixture.conflictEvidence.address,
    neighborState: "reachable",
    neighborMac: "02:00:00:00:00:01",
    activeOwnerDetected: true,
  };
  const plan = build(fixture);
  assert.equal(plan.proxmox.existingGuest.status, "stopped");
  assert.equal(plan.checks.conflictFree, true);
});

function build(fixture) {
  return buildPlan({
    request,
    discovery: clone(fixture.discovery),
    conflictEvidence: clone(fixture.conflictEvidence),
    imagePin: clone(fixture.imagePin),
    managementCidrs: ["130.245.173.10/32"],
    sshPublicKeyFingerprint: "SHA256:fixture-key",
    generatedAt: "2026-08-07T00:00:00.000Z",
  });
}

function assertBlocked(fixture, pattern) {
  assert.throws(
    () => build(fixture),
    (error) => error instanceof PlanBlockedError && error.message.includes(pattern),
  );
}

function guest({ vmId, name, address = "130.245.173.50", mac = "00:11:22:33:44:55" }) {
  return {
    type: "qemu",
    vmId,
    name,
    status: "stopped",
    config: {
      digest: "a".repeat(64),
      name,
      ipConfigurations: [{ key: "ipconfig0", value: `ip=${address}/24,gw=130.245.173.1` }],
      networks: [{ key: "net0", value: `virtio=${mac},bridge=vmbr0` }],
      hardware: {
        bios: "ovmf",
        machine: "q35",
        cpu: "host",
        cores: request.resources.vcpus,
        sockets: request.resources.sockets,
        memoryMiB: request.resources.memoryMiB,
        agent: "enabled=1",
        boot: "order=scsi0",
        disks: [
          { key: "scsi0", value: `vm-zfs:vm-${vmId}-disk-0,size=64G` },
          { key: "scsi1", value: `vm-zfs:vm-${vmId}-disk-1,size=256G` },
        ],
      },
    },
  };
}

function clone(value) {
  return structuredClone(value);
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}
