import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

import {
  ApplyBlockedError,
  assertLivePreconditions,
  DestructiveChangeError,
  destructiveConfirmation,
  nextProvisioningAction,
} from "../public-beta-apply-lib.js";
import { assertValidPlan } from "../public-beta-plan-lib.js";

const root = resolve(import.meta.dirname, "../../..");
const plan = assertValidPlan(
  JSON.parse(
    await readFile(resolve(root, "docs/evidence/public-beta-preflight-plan.json"), "utf8"),
  ),
);
const seedVolume = `local:iso/mako-cloud-public-beta-seed-${plan.planHash}-fixture.iso`;
const imagePath = "/var/lib/vz/template/iso/noble-server-cloudimg-amd64-20260801.img";

test("a dry-run against an absent VM returns creation without executing it", () => {
  const action = next({ current: null });
  assert.equal(action.id, "create-vm");
  assert.equal(action.mutating, true);
  assert.deepEqual(action.command.slice(0, 3), ["qm", "create", "124"]);
  assert(action.command.includes(`Mako Cloud public beta; mako-plan-hash=${plan.planHash}`));
});

test("repeated apply against a matching running VM is non-destructive", () => {
  const action = next({ current: completeVm() });
  assert.deepEqual(action, { id: "converged", mutating: false, command: null });
});

test("partial provisioning resumes one safe step at a time", async (t) => {
  const current = completeVm();
  delete current.config.efidisk0;
  await t.test("creates a missing EFI disk", () => {
    assert.equal(next({ current }).id, "create-efi-disk");
  });
  current.config.efidisk0 = `${plan.storage.os.id}:vm-124-disk-0,efitype=4m,pre-enrolled-keys=1,size=4M`;
  delete current.config.scsi0;
  await t.test("imports a missing OS disk", () => {
    assert.equal(next({ current }).id, "import-os-disk");
  });
  current.config.scsi0 = `${plan.storage.os.id}:vm-124-disk-1,size=3G`;
  await t.test("grows an imported OS image", () => {
    assert.equal(next({ current }).id, "resize-os-disk");
  });
  current.config.scsi0 = `${plan.storage.os.id}:vm-124-disk-1,size=64G`;
  await t.test("adds safe disk flags without replacing the volume", () => {
    const action = next({ current });
    assert.equal(action.id, "configure-os-disk");
    assert(action.command.at(-1).includes(`${plan.storage.os.id}:vm-124-disk-1`));
  });
});

test("a larger or foreign disk stops with a target-specific typed confirmation", () => {
  const current = completeVm();
  current.config.scsi1 = "other-storage:vm-124-disk-2,size=256G";
  assert.throws(
    () => next({ current }),
    (error) =>
      error instanceof DestructiveChangeError &&
      error.confirmation === destructiveConfirmation(plan, "scsi1"),
  );
  assert.throws(
    () =>
      next({
        current,
        destructiveConfirmationValue: destructiveConfirmation(plan, "scsi1"),
      }),
    (error) =>
      error instanceof ApplyBlockedError && /dedicated replacement workflow/.test(error.message),
  );
});

test("a stale plan hash is rejected before an action is built", () => {
  const stale = structuredClone(plan);
  stale.resources.memoryMiB += 1024;
  assert.throws(
    () =>
      nextProvisioningAction({
        plan: stale,
        current: null,
        imagePath,
        seedVolume,
      }),
    /plan hash does not match/,
  );
});

test("a superseded hash-bound cloud-init ISO is replaced without touching persistent disks", () => {
  const current = completeVm();
  current.config.ide2 = "local:iso/older-plan-seed.iso,media=cdrom";
  const action = next({ current });
  assert.equal(action.id, "replace-cloud-init-seed");
  assert.deepEqual(action.command.slice(0, 4), ["qm", "set", "124", "--ide2"]);
  assert.equal(action.command.at(-1), `${seedVolume},media=cdrom`);
});

test("live preflight permits the plan-owned guest and rejects an unexpected address owner", () => {
  const current = completeVm();
  const discovery = liveDiscovery(current);
  const evidence = liveEvidence();
  assert.doesNotThrow(() => assertLivePreconditions(plan, discovery, evidence));
  const collision = structuredClone(evidence);
  collision.address = {
    ...collision.address,
    neighborState: "reachable",
    neighborMac: "aa:bb:cc:dd:ee:ff",
    activeOwnerDetected: true,
  };
  assert.throws(() => assertLivePreconditions(plan, discovery, collision), /unexpected owner/);
});

function next({ current, destructiveConfirmationValue } = {}) {
  return nextProvisioningAction({
    plan,
    current,
    imagePath,
    seedVolume,
    destructiveConfirmationValue,
  });
}

function completeVm() {
  return {
    type: "qemu",
    status: "running",
    config: {
      name: plan.identity.vmName,
      description: `Mako Cloud public beta; mako-plan-hash=${plan.planHash}`,
      bios: "ovmf",
      machine: "q35",
      cpu: "host",
      cores: 8,
      sockets: 1,
      memory: 16384,
      balloon: 0,
      agent: "enabled=1,fstrim_cloned_disks=1,freeze-fs-on-backup=1",
      scsihw: "virtio-scsi-single",
      net0: `virtio=${plan.network.macAddress},bridge=${plan.network.bridge},firewall=1`,
      ipconfig0: `ip=${plan.identity.ipv4Address}/${plan.network.prefixLength},gw=${plan.network.gateway}`,
      ostype: "l26",
      onboot: 1,
      serial0: "socket",
      vga: "serial0",
      tablet: 0,
      hotplug: 0,
      efidisk0: `${plan.storage.os.id}:vm-124-disk-0,efitype=4m,pre-enrolled-keys=1,size=4M`,
      scsi0: `${plan.storage.os.id}:vm-124-disk-1,discard=on,iothread=1,size=64G,ssd=1`,
      scsi1: `${plan.storage.data.id}:vm-124-disk-2,discard=on,iothread=1,size=256G,ssd=1`,
      ide2: `${seedVolume},media=cdrom,size=372K`,
      boot: "order=scsi0",
      protection: 1,
    },
  };
}

function liveDiscovery(current) {
  return {
    cluster: plan.proxmox.cluster,
    selectedNode: { name: plan.proxmox.node },
    networks: [
      {
        bridge: plan.network.bridge,
        prefixLength: plan.network.prefixLength,
        gateway: plan.network.gateway,
      },
    ],
    dns: { servers: plan.network.dnsServers },
    storage: [
      ...new Map(Object.values(plan.storage).map((storage) => [storage.id, storage])).values(),
    ],
    guests: [
      {
        type: "qemu",
        vmId: plan.proxmox.vmId,
        name: plan.identity.vmName,
        config: {
          name: plan.identity.vmName,
          description: current.config.description,
          ipConfigurations: [{ key: "ipconfig0", value: current.config.ipconfig0 }],
          networks: [{ key: "net0", value: current.config.net0 }],
        },
      },
    ],
  };
}

function liveEvidence() {
  return {
    dns: {
      localA: [plan.identity.ipv4Address],
      publicAnswers: [
        { server: "1.1.1.1", a: [plan.identity.ipv4Address], aaaa: [] },
        { server: "8.8.8.8", a: [plan.identity.ipv4Address], aaaa: [] },
      ],
      errors: [],
    },
    address: {
      routeDevice: plan.network.bridge,
      onLink: true,
      neighborState: "incomplete",
      neighborMac: null,
      activeOwnerDetected: false,
      probeConclusive: true,
    },
  };
}
