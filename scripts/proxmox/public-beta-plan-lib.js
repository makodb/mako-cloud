import { createHash } from "node:crypto";
import { isIP } from "node:net";

const secretKeyPattern = /(?:password|passwd|private.?key|secret|token|credential|authorization)/i;
const safeSecurityMetadataKeys = new Set(["passwordLogin", "sshPublicKeyFingerprint"]);
const sha256Pattern = /^[0-9a-f]{64}$/;
const macPattern = /^(?:[0-9a-f]{2}:){5}[0-9a-f]{2}$/i;

export function assertValidPlan(plan, { verifyHash = true } = {}) {
  assert(isObject(plan), "plan must be an object");
  assert(plan.schemaVersion === 1, "plan schemaVersion must be 1");
  assert(!Number.isNaN(Date.parse(plan.generatedAt)), "plan generatedAt must be an ISO date-time");
  assert(plan.mode === "read-only-plan", "plan mode must be read-only-plan");
  assertExactKeys(plan.identity, ["fqdn", "guestType", "ipv4Address", "vmName"], "identity");
  assert(plan.identity.fqdn === "cloud-test.makodb.com", "unexpected public hostname");
  assert(plan.identity.ipv4Address === "130.245.173.11", "unexpected public address");
  assert(plan.identity.guestType === "qemu", "the beta environment must be a VM");
  assert(plan.identity.vmName === "mako-cloud-public-beta", "unexpected VM name");

  assertNonEmpty(plan.proxmox?.cluster, "Proxmox cluster");
  assertNonEmpty(plan.proxmox?.node, "Proxmox node");
  assertInteger(plan.proxmox?.vmId, 100, 999_999_999, "VM identifier");
  assert(
    plan.proxmox.existingGuest === null || isObject(plan.proxmox.existingGuest),
    "existingGuest must be null or an object",
  );

  assertInteger(plan.resources?.vcpus, 1, undefined, "vCPU count");
  assertInteger(plan.resources?.sockets, 1, undefined, "socket count");
  assertNonEmpty(plan.resources?.cpuType, "CPU type");
  assertInteger(plan.resources?.memoryMiB, 2048, undefined, "memory");
  assertInteger(plan.resources?.osDiskGiB, 16, undefined, "OS disk");
  assertInteger(plan.resources?.dataDiskGiB, 32, undefined, "data disk");

  assert(plan.image?.distribution === "ubuntu", "guest image must be Ubuntu");
  assert(plan.image?.release === "24.04", "guest image must be Ubuntu 24.04");
  assert(plan.image?.architecture === "amd64", "guest image must be amd64");
  assert(/^https:\/\//.test(plan.image?.source), "guest image source must use HTTPS");
  assert(sha256Pattern.test(plan.image?.sha256), "guest image SHA-256 is invalid");
  assert(typeof plan.image?.availableLocally === "boolean", "image availability must be known");

  assertNonEmpty(plan.network?.bridge, "network bridge");
  assertInteger(plan.network?.prefixLength, 1, 32, "IPv4 prefix length");
  assert(isIP(plan.network?.gateway) !== 0, "gateway must be an IP address");
  assertUniqueStrings(plan.network?.dnsServers, "DNS servers");
  assert(
    plan.network.dnsServers.every((value) => isIP(value) !== 0),
    "DNS servers must be IP addresses",
  );
  assert(macPattern.test(plan.network?.macAddress), "MAC address is invalid");
  assert(["disabled", "configured"].includes(plan.network?.ipv6Mode), "IPv6 mode is invalid");
  assert(plan.network?.onLink === true, "target address must be on-link");

  for (const role of ["os", "data", "backup"]) validateStorage(plan.storage?.[role], role);
  assert(
    plan.storage.backup.offVm === true,
    "backup storage must be outside the VM failure domain",
  );
  assertUniqueStrings(plan.security?.managementCidrs, "management CIDRs");
  assertNonEmpty(plan.security?.sshPublicKeyFingerprint, "SSH public-key fingerprint");
  assert(plan.security?.passwordLogin === false, "password login must be disabled");
  assert(plan.security?.rootLogin === false, "direct root login must be disabled");

  assertUniqueStrings(plan.checks?.dns?.localA ?? [], "local A records", true);
  assertUniqueStrings(plan.checks?.dns?.publicA ?? [], "public A records", true);
  assertUniqueStrings(plan.checks?.dns?.publicAaaa ?? [], "public AAAA records", true);
  assert(
    plan.checks.dns.localA.every((value) => isIP(value) !== 0),
    "local A records are invalid",
  );
  assert(
    plan.checks.dns.publicA.every((value) => isIP(value) !== 0),
    "public A records are invalid",
  );
  assert(
    plan.checks.dns.publicAaaa.every((value) => isIP(value) !== 0),
    "public AAAA records are invalid",
  );
  assert(
    Array.isArray(plan.checks?.inventory?.vmNameMatches),
    "VM-name inventory matches are required",
  );
  assert(
    Array.isArray(plan.checks?.inventory?.addressMatches),
    "address inventory matches are required",
  );
  assert(
    typeof plan.checks?.inventory?.vmIdAvailable === "boolean",
    "VM-id availability is required",
  );
  assertNonEmpty(plan.checks?.address?.routeDevice, "address route device");
  assert(
    [
      "absent",
      "failed",
      "incomplete",
      "reachable",
      "stale",
      "delay",
      "probe",
      "permanent",
      "unknown",
    ].includes(plan.checks?.address?.neighborState),
    "neighbor state is invalid",
  );
  assert(
    typeof plan.checks?.address?.activeOwnerDetected === "boolean",
    "address-owner result is required",
  );
  for (const key of ["memoryPass", "osStoragePass", "dataStoragePass", "backupStoragePass"]) {
    assert(typeof plan.checks?.capacity?.[key] === "boolean", `capacity.${key} must be boolean`);
  }
  assert(Array.isArray(plan.checks?.ambiguities), "plan ambiguities are required");
  assert(typeof plan.checks?.conflictFree === "boolean", "conflict-free result is required");

  assert(
    ["create", "noop", "repair", "destructive"].includes(plan.changes?.classification),
    "change classification is invalid",
  );
  assert(Array.isArray(plan.changes?.items), "change items are required");
  assert(
    typeof plan.changes?.requiresDestructiveConfirmation === "boolean",
    "destructive-confirmation classification is required",
  );
  for (const item of plan.changes.items) {
    assertNonEmpty(item?.path, "change path");
    assert(
      ["create", "preserve", "update", "replace", "delete"].includes(item?.action),
      "change action is invalid",
    );
    assert(typeof item?.destructive === "boolean", "change destructive flag is required");
  }

  assertNoSecrets(plan);
  assert(sha256Pattern.test(plan.planHash), "plan hash must be SHA-256");
  if (verifyHash)
    assert(plan.planHash === computePlanHash(plan), "plan hash does not match plan content");
  return plan;
}

export function computePlanHash(plan) {
  const hashable = structuredClone(plan);
  delete hashable.planHash;
  return createHash("sha256").update(canonicalJson(hashable)).digest("hex");
}

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (isObject(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function assertNoSecrets(value, path = "plan") {
  if (Array.isArray(value)) {
    for (const [index, entry] of value.entries()) {
      assertNoSecrets(entry, `${path}[${index}]`);
    }
    return;
  }
  if (!isObject(value)) return;
  for (const [key, entry] of Object.entries(value)) {
    assert(
      safeSecurityMetadataKeys.has(key) || !secretKeyPattern.test(key),
      `${path}.${key} is a forbidden secret-bearing field`,
    );
    assertNoSecrets(entry, `${path}.${key}`);
  }
}

function validateStorage(storage, role) {
  assertNonEmpty(storage?.id, `${role} storage id`);
  assertUniqueStrings(storage?.content, `${role} storage content`);
  assertInteger(storage?.availableBytes, 0, undefined, `${role} available bytes`);
  assertInteger(storage?.requiredBytes, 1, undefined, `${role} required bytes`);
  assert(
    Number.isFinite(storage?.headroomPercent) &&
      storage.headroomPercent >= 0 &&
      storage.headroomPercent <= 100,
    `${role} storage headroom is invalid`,
  );
  assert(typeof storage?.offVm === "boolean", `${role} offVm must be boolean`);
}

function assertExactKeys(value, keys, name) {
  assert(isObject(value), `${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `${name} fields do not match the schema`,
  );
}

function assertUniqueStrings(value, name, allowEmpty = false) {
  assert(
    Array.isArray(value) && (allowEmpty || value.length > 0),
    `${name} must be a non-empty array`,
  );
  assert(
    value.every((entry) => typeof entry === "string" && entry !== ""),
    `${name} must contain strings`,
  );
  assert(new Set(value).size === value.length, `${name} must be unique`);
}

function assertInteger(value, minimum, maximum, name) {
  assert(Number.isInteger(value) && value >= minimum, `${name} is below its minimum`);
  if (maximum !== undefined) assert(value <= maximum, `${name} exceeds its maximum`);
}

function assertNonEmpty(value, name) {
  assert(typeof value === "string" && value.trim() !== "", `${name} must be non-empty`);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
