import { createHash } from "node:crypto";

import { assertValidPlan, canonicalJson } from "./public-beta-plan-lib.js";

const markerPrefix = "mako-plan-hash=";
const diskOptions = Object.freeze({ discard: "on", iothread: "1", ssd: "1" });

export class ApplyBlockedError extends Error {
  constructor(message) {
    super(message);
    this.name = "ApplyBlockedError";
  }
}

export class DestructiveChangeError extends ApplyBlockedError {
  constructor(message, confirmation) {
    super(`${message}. Regenerate and review the plan, then pass exactly: ${confirmation}`);
    this.name = "DestructiveChangeError";
    this.confirmation = confirmation;
  }
}

export function planMarker(plan) {
  return `${markerPrefix}${assertValidPlan(plan).planHash}`;
}

export function destructiveConfirmation(plan, target) {
  assertSafeTarget(target);
  return `REPLACE ${target} ON VM ${plan.proxmox.vmId} FOR PLAN ${plan.planHash}`;
}

export function assertLivePreconditions(plan, discovery, evidence) {
  assertValidPlan(plan);
  const blockers = [];
  if (discovery.cluster !== plan.proxmox.cluster) {
    blockers.push(`cluster changed from ${plan.proxmox.cluster} to ${discovery.cluster}`);
  }
  if (discovery.selectedNode.name !== plan.proxmox.node) {
    blockers.push(
      `selected node changed from ${plan.proxmox.node} to ${discovery.selectedNode.name}`,
    );
  }
  const bridge = discovery.networks.find(({ bridge: name }) => name === plan.network.bridge);
  if (bridge === undefined) blockers.push(`bridge ${plan.network.bridge} is no longer active`);
  else {
    if (bridge.prefixLength !== plan.network.prefixLength) {
      blockers.push(
        `bridge prefix changed from /${plan.network.prefixLength} to /${bridge.prefixLength}`,
      );
    }
    if (bridge.gateway !== plan.network.gateway) {
      blockers.push(`bridge gateway changed from ${plan.network.gateway} to ${bridge.gateway}`);
    }
  }
  if (!sameSet(discovery.dns.servers, plan.network.dnsServers)) {
    blockers.push("Proxmox DNS servers changed from the recorded plan");
  }
  for (const role of ["os", "data", "backup"]) {
    const desired = plan.storage[role];
    const current = discovery.storage.find(({ id }) => id === desired.id);
    if (current === undefined) blockers.push(`${role} storage ${desired.id} is no longer active`);
    else if (!desired.content.every((content) => current.content.includes(content))) {
      blockers.push(`${role} storage ${desired.id} lost a required content capability`);
    }
  }

  const expectedA = [plan.identity.ipv4Address];
  if (!sameSet(evidence.dns.localA, expectedA)) {
    blockers.push(`local DNS no longer resolves only to ${plan.identity.ipv4Address}`);
  }
  for (const answer of evidence.dns.publicAnswers) {
    if (!sameSet(answer.a, expectedA)) {
      blockers.push(`${answer.server} no longer resolves only to ${plan.identity.ipv4Address}`);
    }
    if (plan.network.ipv6Mode === "disabled" && answer.aaaa.length > 0) {
      blockers.push(`${answer.server} now returns an unexpected AAAA record`);
    }
  }
  blockers.push(...evidence.dns.errors);
  if (evidence.address.routeDevice !== plan.network.bridge || evidence.address.onLink !== true) {
    blockers.push(`address route no longer uses on-link bridge ${plan.network.bridge}`);
  }
  if (evidence.address.probeConclusive !== true) blockers.push("address probe is inconclusive");

  const guestsByName = discovery.guests.filter(
    (guest) => guest.name === plan.identity.vmName || guest.config.name === plan.identity.vmName,
  );
  const guestsById = discovery.guests.filter((guest) => guest.vmId === plan.proxmox.vmId);
  const guestsByAddress = discovery.guests.filter((guest) =>
    guest.config.ipConfigurations.some(({ value }) => value.includes(plan.identity.ipv4Address)),
  );
  if (guestsByName.some((guest) => guest.vmId !== plan.proxmox.vmId)) {
    blockers.push(`VM name ${plan.identity.vmName} moved to another guest`);
  }
  if (guestsByAddress.some((guest) => guest.vmId !== plan.proxmox.vmId)) {
    blockers.push(`address ${plan.identity.ipv4Address} is assigned to another guest`);
  }
  const expectedGuest = guestsById[0] ?? null;
  if (expectedGuest !== null) {
    if (expectedGuest.type !== "qemu" || expectedGuest.name !== plan.identity.vmName) {
      blockers.push(`VM identifier ${plan.proxmox.vmId} is owned by another resource`);
    }
    if (!expectedGuest.config.description?.includes(planMarker(plan))) {
      blockers.push(`VM identifier ${plan.proxmox.vmId} lacks the recorded plan marker`);
    }
  }
  if (evidence.address.activeOwnerDetected) {
    const macMatches =
      expectedGuest?.config.networks.some(({ value }) =>
        value.toLowerCase().includes(plan.network.macAddress.toLowerCase()),
      ) && evidence.address.neighborMac?.toLowerCase() === plan.network.macAddress.toLowerCase();
    if (!macMatches) blockers.push(`address ${plan.identity.ipv4Address} has an unexpected owner`);
  }
  if (blockers.length > 0) {
    throw new ApplyBlockedError(`live preflight rejected mutation: ${blockers.join("; ")}`);
  }
}

export function nextProvisioningAction({
  plan,
  current,
  imagePath,
  seedVolume,
  destructiveConfirmationValue,
}) {
  assertValidPlan(plan);
  assertAbsolutePath(imagePath, "image path");
  assertVolume(seedVolume, "seed volume");
  if (current === null) return createVmAction(plan);
  assertOwnedVm(plan, current);
  assertBaseConfiguration(plan, current.config);

  if (current.config.efidisk0 === undefined) {
    return commandAction("create-efi-disk", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--efidisk0",
      `${plan.storage.os.id}:0,efitype=4m,pre-enrolled-keys=1`,
    ]);
  }
  assertDiskStorage(
    plan,
    current.config.efidisk0,
    "efidisk0",
    plan.storage.os.id,
    destructiveConfirmationValue,
  );

  if (current.config.scsi0 === undefined) {
    return commandAction("import-os-disk", [
      "qm",
      "disk",
      "import",
      String(plan.proxmox.vmId),
      imagePath,
      plan.storage.os.id,
      "--format",
      "raw",
      "--target-disk",
      "scsi0",
    ]);
  }
  const osDisk = assertDiskStorage(
    plan,
    current.config.scsi0,
    "scsi0",
    plan.storage.os.id,
    destructiveConfirmationValue,
  );
  const osSize = diskSizeGiB(osDisk);
  if (osSize > plan.resources.osDiskGiB) {
    throw destructiveError(
      plan,
      "scsi0",
      `OS disk is ${osSize} GiB, larger than the planned ${plan.resources.osDiskGiB} GiB`,
    );
  }
  if (osSize < plan.resources.osDiskGiB) {
    return commandAction("resize-os-disk", [
      "qm",
      "disk",
      "resize",
      String(plan.proxmox.vmId),
      "scsi0",
      `${plan.resources.osDiskGiB}G`,
    ]);
  }
  if (!hasDiskOptions(osDisk)) {
    return commandAction("configure-os-disk", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--scsi0",
      withDiskOptions(osDisk),
    ]);
  }

  if (current.config.scsi1 === undefined) {
    return commandAction("create-data-disk", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--scsi1",
      `${plan.storage.data.id}:${plan.resources.dataDiskGiB},discard=on,iothread=1,ssd=1`,
    ]);
  }
  const dataDisk = assertDiskStorage(
    plan,
    current.config.scsi1,
    "scsi1",
    plan.storage.data.id,
    destructiveConfirmationValue,
  );
  const dataSize = diskSizeGiB(dataDisk);
  if (dataSize !== plan.resources.dataDiskGiB) {
    if (dataSize < plan.resources.dataDiskGiB) {
      return commandAction("resize-data-disk", [
        "qm",
        "disk",
        "resize",
        String(plan.proxmox.vmId),
        "scsi1",
        `${plan.resources.dataDiskGiB}G`,
      ]);
    }
    throw destructiveError(
      plan,
      "scsi1",
      `data disk is ${dataSize} GiB, larger than the planned ${plan.resources.dataDiskGiB} GiB`,
    );
  }
  if (!hasDiskOptions(dataDisk)) {
    return commandAction("configure-data-disk", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--scsi1",
      withDiskOptions(dataDisk),
    ]);
  }

  if (current.config.ide2 === undefined) {
    return commandAction("attach-cloud-init-seed", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--ide2",
      `${seedVolume},media=cdrom`,
    ]);
  }
  if (String(current.config.ide2).split(",", 1)[0] !== seedVolume) {
    return commandAction("replace-cloud-init-seed", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--ide2",
      `${seedVolume},media=cdrom`,
    ]);
  }
  if (String(current.config.boot ?? "") !== "order=scsi0") {
    return commandAction("set-boot-order", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--boot",
      "order=scsi0",
    ]);
  }
  if (!truthy(current.config.protection)) {
    return commandAction("protect-vm", [
      "qm",
      "set",
      String(plan.proxmox.vmId),
      "--protection",
      "1",
    ]);
  }
  if (current.status === "stopped") {
    return commandAction("start-vm", ["qm", "start", String(plan.proxmox.vmId)]);
  }
  if (current.status !== "running") {
    throw new ApplyBlockedError(
      `VM status is ${current.status}; refusing to guess a recovery action`,
    );
  }
  return { id: "converged", mutating: false, command: null };
}

export function sanitizedVmEvidence(plan, current, seedSha256) {
  assertOwnedVm(plan, current);
  const config = current.config;
  return {
    schemaVersion: 1,
    capturedAt: new Date().toISOString(),
    planHash: plan.planHash,
    node: plan.proxmox.node,
    vmId: plan.proxmox.vmId,
    status: current.status,
    identity: { name: config.name, descriptionMarker: planMarker(plan) },
    hardware: {
      bios: config.bios,
      machine: config.machine,
      cpu: config.cpu,
      cores: Number(config.cores),
      sockets: Number(config.sockets),
      memoryMiB: Number(config.memory),
      scsihw: config.scsihw,
      agent: config.agent,
    },
    disks: Object.fromEntries(
      ["efidisk0", "scsi0", "scsi1", "ide2"]
        .filter((key) => config[key] !== undefined)
        .map((key) => [key, String(config[key])]),
    ),
    network: { net0: config.net0, ipconfig0: config.ipconfig0 },
    seedSha256,
    configDigest: createHash("sha256").update(canonicalJson(config)).digest("hex"),
  };
}

function createVmAction(plan) {
  return commandAction("create-vm", [
    "qm",
    "create",
    String(plan.proxmox.vmId),
    "--name",
    plan.identity.vmName,
    "--description",
    `Mako Cloud public beta; ${planMarker(plan)}`,
    "--tags",
    `mako-cloud;public-beta;plan-${plan.planHash.slice(0, 12)}`,
    "--machine",
    "q35",
    "--bios",
    "ovmf",
    "--cpu",
    plan.resources.cpuType,
    "--cores",
    String(plan.resources.vcpus),
    "--sockets",
    String(plan.resources.sockets),
    "--memory",
    String(plan.resources.memoryMiB),
    "--balloon",
    "0",
    "--agent",
    "enabled=1,freeze-fs=1,fstrim_cloned_disks=1",
    "--scsihw",
    "virtio-scsi-single",
    "--net0",
    `virtio=${plan.network.macAddress},bridge=${plan.network.bridge},firewall=1`,
    "--ipconfig0",
    `ip=${plan.identity.ipv4Address}/${plan.network.prefixLength},gw=${plan.network.gateway}`,
    "--nameserver",
    plan.network.dnsServers.join(" "),
    "--ostype",
    "l26",
    "--onboot",
    "1",
    "--serial0",
    "socket",
    "--vga",
    "serial0",
    "--tablet",
    "0",
    "--hotplug",
    "0",
  ]);
}

function assertOwnedVm(plan, current) {
  if (current === null || current.type !== "qemu") {
    throw new ApplyBlockedError(`VM identifier ${plan.proxmox.vmId} is not the planned QEMU guest`);
  }
  if (current.config.name !== plan.identity.vmName) {
    throw new ApplyBlockedError(
      `VM identifier ${plan.proxmox.vmId} is named ${current.config.name}`,
    );
  }
  if (!String(current.config.description ?? "").includes(planMarker(plan))) {
    throw new ApplyBlockedError(
      `VM identifier ${plan.proxmox.vmId} is not owned by plan ${plan.planHash}`,
    );
  }
}

function assertBaseConfiguration(plan, config) {
  const destructive = [
    ["bios", config.bios, "ovmf"],
    ["machine", config.machine, "q35"],
  ];
  for (const [field, current, expected] of destructive) {
    if (String(current) !== expected) {
      throw destructiveError(
        plan,
        "VM",
        `${field} is ${current ?? "absent"}, expected ${expected}`,
      );
    }
  }
  const expected = {
    name: plan.identity.vmName,
    cpu: plan.resources.cpuType,
    cores: plan.resources.vcpus,
    sockets: plan.resources.sockets,
    memory: plan.resources.memoryMiB,
    balloon: 0,
    scsihw: "virtio-scsi-single",
    ostype: "l26",
    onboot: 1,
    serial0: "socket",
    vga: "serial0",
    tablet: 0,
    hotplug: 0,
  };
  for (const [field, value] of Object.entries(expected)) {
    if (String(config[field]) !== String(value)) {
      throw new ApplyBlockedError(
        `${field} drifted: found ${config[field] ?? "absent"}, expected ${value}`,
      );
    }
  }
  const network = properties(config.net0);
  if (
    network.positional.toLowerCase() !== `virtio=${plan.network.macAddress}`.toLowerCase() ||
    network.bridge !== plan.network.bridge ||
    network.firewall !== "1"
  ) {
    throw new ApplyBlockedError("net0 differs from the hash-bound network configuration");
  }
  const ip = properties(config.ipconfig0);
  if (
    ip.ip !== `${plan.identity.ipv4Address}/${plan.network.prefixLength}` ||
    ip.gw !== plan.network.gateway
  ) {
    throw new ApplyBlockedError("ipconfig0 differs from the hash-bound static address");
  }
  const agent = properties(config.agent);
  const freezeFileSystems = agent["freeze-fs-on-backup"] === "1" || agent["freeze-fs"] === "1";
  if (
    !["1", "enabled=1"].includes(agent.positional) ||
    agent.fstrim_cloned_disks !== "1" ||
    !freezeFileSystems
  ) {
    throw new ApplyBlockedError("QEMU guest-agent settings differ from the plan");
  }
}

function assertDiskStorage(plan, value, target, storageId, providedConfirmation) {
  const disk = String(value);
  if (disk.split(":", 1)[0] !== storageId) {
    const error = destructiveError(
      plan,
      target,
      `${target} uses storage ${disk.split(":", 1)[0]}, expected ${storageId}`,
    );
    if (providedConfirmation !== error.confirmation) throw error;
    throw new ApplyBlockedError(
      `typed confirmation accepted for ${target}, but ordinary apply preserves the existing volume; use the dedicated replacement workflow`,
    );
  }
  return disk;
}

function destructiveError(plan, target, message) {
  return new DestructiveChangeError(message, destructiveConfirmation(plan, target));
}

function diskSizeGiB(value) {
  const match = String(value).match(/(?:^|,)size=(\d+(?:\.\d+)?)([KMGT])(?:,|$)/i);
  if (match === null) throw new ApplyBlockedError(`disk size is absent from ${value}`);
  const magnitude = Number(match[1]);
  const factors = { K: 1 / 1024 ** 2, M: 1 / 1024, G: 1, T: 1024 };
  return magnitude * factors[match[2].toUpperCase()];
}

function hasDiskOptions(value) {
  const parsed = properties(value);
  return Object.entries(diskOptions).every(([key, expected]) => parsed[key] === expected);
}

function withDiskOptions(value) {
  const entries = String(value)
    .split(",")
    .filter((entry) => !entry.startsWith("size="));
  for (const [key, option] of Object.entries(diskOptions)) {
    const index = entries.findIndex((entry) => entry.startsWith(`${key}=`));
    if (index === -1) entries.push(`${key}=${option}`);
    else entries[index] = `${key}=${option}`;
  }
  return entries.join(",");
}

function properties(value) {
  const [positional = "", ...parts] = String(value ?? "").split(",");
  const result = { positional };
  const firstSeparator = positional.indexOf("=");
  if (firstSeparator > 0) {
    result[positional.slice(0, firstSeparator)] = positional.slice(firstSeparator + 1);
  }
  for (const part of parts) {
    const separator = part.indexOf("=");
    if (separator > 0) result[part.slice(0, separator)] = part.slice(separator + 1);
  }
  return result;
}

function commandAction(id, command) {
  return { id, mutating: true, command };
}

function assertVolume(value, name) {
  if (!/^[a-zA-Z0-9_.-]+:[a-zA-Z0-9_./-]+$/.test(value)) {
    throw new ApplyBlockedError(`${name} is not a safe Proxmox volume identifier`);
  }
}

function assertAbsolutePath(value, name) {
  if (!/^\/[a-zA-Z0-9_./-]+$/.test(value)) {
    throw new ApplyBlockedError(`${name} is not a safe absolute path`);
  }
}

function assertSafeTarget(value) {
  if (!/^(?:VM|efidisk0|ide2|scsi0|scsi1)$/.test(value)) throw new Error("unsafe target");
}

function sameSet(left, right) {
  return JSON.stringify([...left].sort()) === JSON.stringify([...right].sort());
}

function truthy(value) {
  return value === true || value === 1 || value === "1";
}
