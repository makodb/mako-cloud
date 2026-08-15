import { createHash } from "node:crypto";

import { evaluateConflicts } from "./public-beta-conflicts.js";
import {
  assertNoSecrets,
  assertValidPlan,
  canonicalJson,
  computePlanHash,
} from "./public-beta-plan-lib.js";

const gib = 1024 ** 3;

export function buildPlan({
  request,
  discovery,
  conflictEvidence,
  imagePin,
  managementCidrs,
  sshPublicKeyFingerprint,
  storageSelections = {},
  generatedAt = new Date().toISOString(),
}) {
  const conflict = evaluateConflicts({ request, discovery, evidence: conflictEvidence });
  const ambiguities = [...conflict.checks.ambiguities];
  const memoryPass =
    discovery.selectedNode.memoryBytes -
      discovery.selectedNode.memoryUsedBytes -
      request.resources.memoryMiB * 1024 ** 2 >=
    request.resources.minimumHostFreeMemoryMiBAfterAllocation * 1024 ** 2;
  if (!memoryPass)
    ambiguities.push("host memory would fall below the required post-allocation reserve");

  const os = selectStorage({
    discovery,
    requestedId: storageSelections.os,
    purpose: "OS",
    requiredContent: "images",
    requiredBytes: request.resources.osDiskGiB * gib,
    minimumHeadroomPercent: request.resources.minimumStorageHeadroomPercent,
    offVm: false,
    ambiguities,
  });
  const data = selectStorage({
    discovery,
    requestedId: storageSelections.data,
    purpose: "data",
    requiredContent: "images",
    requiredBytes: request.resources.dataDiskGiB * gib,
    minimumHeadroomPercent: request.resources.minimumStorageHeadroomPercent,
    offVm: false,
    ambiguities,
  });
  const backup = selectStorage({
    discovery,
    requestedId: storageSelections.backup,
    purpose: "backup",
    requiredContent: "backup",
    requiredBytes: request.resources.dataDiskGiB * gib,
    minimumHeadroomPercent: request.resources.minimumStorageHeadroomPercent,
    offVm: true,
    ambiguities,
  });

  assertImagePin(imagePin, request.operatingSystem);
  assertManagementCidrs(managementCidrs);
  assert(
    typeof sshPublicKeyFingerprint === "string" && sshPublicKeyFingerprint.length >= 8,
    "an SSH public-key fingerprint is required",
  );
  if (discovery.dns.servers.length === 0) ambiguities.push("Proxmox node DNS servers are absent");
  if (conflict.selectedBridge?.prefixLength === null) ambiguities.push("network prefix is absent");

  const storage = { os, data, backup };
  const capacity = {
    memoryPass,
    osStoragePass: storagePass(os, request.resources.minimumStorageHeadroomPercent),
    dataStoragePass: storagePass(data, request.resources.minimumStorageHeadroomPercent),
    backupStoragePass: storagePass(backup, request.resources.minimumStorageHeadroomPercent),
  };
  for (const [key, pass] of Object.entries(capacity)) {
    if (!pass && !ambiguities.some((message) => message.includes(key))) {
      ambiguities.push(`${key} failed`);
    }
  }

  const changes = classifyChanges({
    existingGuest: conflict.expectedGuest,
    request,
    storage,
  });
  if (changes.requiresDestructiveConfirmation) {
    ambiguities.push("existing guest differences require explicit destructive confirmation");
  }
  const conflictFree = ambiguities.length === 0;
  const plan = {
    schemaVersion: 1,
    generatedAt,
    mode: "read-only-plan",
    identity: structuredClone(request.identity),
    proxmox: {
      cluster: discovery.cluster,
      node: discovery.selectedNode.name,
      vmId: conflict.proposedVmId,
      existingGuest:
        conflict.expectedGuest === null
          ? null
          : {
              node: discovery.selectedNode.name,
              status: conflict.expectedGuest.status,
              configDigest: createHash("sha256")
                .update(canonicalJson(conflict.expectedGuest.config))
                .digest("hex"),
            },
    },
    resources: {
      vcpus: request.resources.vcpus,
      sockets: request.resources.sockets,
      cpuType: request.resources.cpuType,
      memoryMiB: request.resources.memoryMiB,
      osDiskGiB: request.resources.osDiskGiB,
      dataDiskGiB: request.resources.dataDiskGiB,
    },
    image: {
      distribution: request.operatingSystem.distribution,
      release: request.operatingSystem.release,
      architecture: request.operatingSystem.architecture,
      source: imagePin.source,
      sha256: imagePin.sha256,
      availableLocally: discovery.approvedImageCandidates.some(
        (candidate) => candidate.volume === imagePin.localVolume,
      ),
    },
    network: {
      bridge: conflict.selectedBridge?.bridge ?? "unresolved",
      prefixLength: conflict.selectedBridge?.prefixLength ?? 0,
      gateway: conflict.selectedBridge?.gateway ?? "unresolved",
      dnsServers: discovery.dns.servers,
      macAddress: deterministicMac(request.identity.fqdn, conflict.proposedVmId),
      ipv6Mode: request.networkPolicy.ipv6Mode,
      onLink: conflictEvidence.address.onLink,
    },
    storage,
    security: {
      managementCidrs: [...managementCidrs].sort(),
      sshPublicKeyFingerprint,
      passwordLogin: false,
      rootLogin: false,
    },
    checks: {
      ...conflict.checks,
      capacity,
      ambiguities,
      conflictFree,
    },
    changes,
    planHash: "0".repeat(64),
  };
  plan.planHash = computePlanHash(plan);
  assertNoSecrets(plan);
  if (!conflictFree) {
    throw new PlanBlockedError("public beta plan is blocked", ambiguities, plan);
  }
  return assertValidPlan(plan);
}

export function classifyChanges({ existingGuest, request, storage }) {
  if (existingGuest === null) {
    return {
      classification: "create",
      items: [
        { path: "vm", action: "create", destructive: false },
        { path: "vm.osDisk", action: "create", destructive: false },
        { path: "vm.dataDisk", action: "create", destructive: false },
      ],
      requiresDestructiveConfirmation: false,
    };
  }

  const items = [{ path: "vm.identity", action: "preserve", destructive: false }];
  compareHardware(items, existingGuest.config.hardware, request.resources);
  compareDisk(
    items,
    "osDisk",
    existingGuest.config.hardware.disks,
    storage.os.id,
    request.resources.osDiskGiB,
  );
  compareDisk(
    items,
    "dataDisk",
    existingGuest.config.hardware.disks,
    storage.data.id,
    request.resources.dataDiskGiB,
  );
  const changed = items.filter((item) => item.action !== "preserve");
  const destructive = changed.some((item) => item.destructive);
  return {
    classification: destructive ? "destructive" : changed.length === 0 ? "noop" : "repair",
    items,
    requiresDestructiveConfirmation: destructive,
  };
}

export class PlanBlockedError extends Error {
  constructor(message, blockers, plan) {
    super(`${message}: ${blockers.join("; ")}`);
    this.name = "PlanBlockedError";
    this.blockers = blockers;
    this.plan = plan;
  }
}

function selectStorage({
  discovery,
  requestedId,
  purpose,
  requiredContent,
  requiredBytes,
  minimumHeadroomPercent,
  offVm,
  ambiguities,
}) {
  const eligible = discovery.storage.filter(
    (entry) =>
      entry.content.includes(requiredContent) &&
      // Proxmox-managed backup content is outside the guest and survives VM
      // recreation even when it is node-local. Host-loss durability remains a
      // separately measured beta-gate property rather than an invented claim.
      (!offVm || requiredContent === "backup"),
  );
  const selected = requestedId
    ? eligible.find((entry) => entry.id === requestedId)
    : eligible.length === 1
      ? eligible[0]
      : undefined;
  if (requestedId && selected === undefined) {
    ambiguities.push(
      `${purpose} storage ${requestedId} is unavailable or lacks ${requiredContent}`,
    );
  } else if (!requestedId && eligible.length !== 1) {
    ambiguities.push(
      `${purpose} storage is ambiguous; eligible ids: ${eligible.map((entry) => entry.id).join(", ") || "<none>"}`,
    );
  }
  const fallback = selected ?? {
    id: `unresolved-${purpose.toLowerCase()}`,
    content: [requiredContent],
    availableBytes: 0,
    totalBytes: 1,
  };
  const result = {
    id: fallback.id,
    content: fallback.content,
    availableBytes: fallback.availableBytes,
    requiredBytes,
    headroomPercent: remainingHeadroom(fallback, requiredBytes),
    offVm,
  };
  if (selected !== undefined && result.headroomPercent < minimumHeadroomPercent) {
    ambiguities.push(
      `${purpose} storage ${selected.id} would have ${result.headroomPercent.toFixed(2)}% headroom, below ${minimumHeadroomPercent}%`,
    );
  }
  return result;
}

function storagePass(storage, minimumHeadroomPercent) {
  return (
    storage.availableBytes >= storage.requiredBytes &&
    storage.headroomPercent >= minimumHeadroomPercent &&
    (!storage.offVm || storage.id !== "unresolved-backup")
  );
}

function remainingHeadroom(storage, requiredBytes) {
  if (storage.totalBytes <= 0) return 0;
  return Math.max(0, ((storage.availableBytes - requiredBytes) / storage.totalBytes) * 100);
}

function compareHardware(items, existing, desired) {
  for (const [path, current, target] of [
    ["vm.vcpus", existing.cores, desired.vcpus],
    ["vm.sockets", existing.sockets, desired.sockets],
    ["vm.memoryMiB", existing.memoryMiB, desired.memoryMiB],
  ]) {
    items.push({
      path,
      action: current === target ? "preserve" : "update",
      destructive: false,
    });
  }
  for (const [path, current, target] of [
    ["vm.bios", existing.bios, "ovmf"],
    ["vm.machine", existing.machine, "q35"],
  ]) {
    items.push({
      path,
      action: current === target ? "preserve" : "replace",
      destructive: current !== target,
    });
  }
}

function compareDisk(items, name, disks, storageId, sizeGiB) {
  const index = name === "osDisk" ? 0 : 1;
  const disk = disks.find(({ key }) => key === `scsi${index}`);
  if (disk === undefined) {
    items.push({ path: `vm.${name}`, action: "create", destructive: false });
    return;
  }
  const currentStorage = disk.value.split(":", 1)[0];
  const currentSize = Number(disk.value.match(/(?:^|,)size=(\d+(?:\.\d+)?)G(?:,|$)/)?.[1]);
  if (currentStorage !== storageId || !Number.isFinite(currentSize) || currentSize > sizeGiB) {
    items.push({ path: `vm.${name}`, action: "replace", destructive: true });
    return;
  }
  items.push({
    path: `vm.${name}`,
    action: currentSize === sizeGiB ? "preserve" : "update",
    destructive: false,
  });
}

function deterministicMac(fqdn, vmId) {
  const bytes = createHash("sha256").update(`${fqdn}:${vmId}`).digest().subarray(0, 5);
  return `02:${[...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join(":")}`;
}

function assertImagePin(imagePin, operatingSystem) {
  assert(imagePin?.distribution === operatingSystem.distribution, "image distribution mismatch");
  assert(imagePin?.release === operatingSystem.release, "image release mismatch");
  assert(imagePin?.architecture === operatingSystem.architecture, "image architecture mismatch");
  assert(/^https:\/\//.test(imagePin?.source), "image source must use HTTPS");
  assert(/^[0-9a-f]{64}$/.test(imagePin?.sha256), "image SHA-256 is required");
}

function assertManagementCidrs(cidrs) {
  assert(Array.isArray(cidrs) && cidrs.length > 0, "at least one management CIDR is required");
  assert(
    cidrs.every((cidr) => /^[0-9a-fA-F:.]+\/\d{1,3}$/.test(cidr)),
    "management CIDR is invalid",
  );
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
