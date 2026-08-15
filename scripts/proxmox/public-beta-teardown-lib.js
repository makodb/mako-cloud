import { createHash } from "node:crypto";

import { canonicalJson } from "./public-beta-plan-lib.js";

const DIGEST = /^[0-9a-f]{64}$/;

export function buildTeardownPlan(discovery, generatedAt = new Date().toISOString()) {
  assert(discovery?.vm?.id === 124, "teardown discovery must identify VM 124");
  assert(
    discovery.vm.status === "running" || discovery.vm.status === "stopped",
    "VM status is invalid",
  );
  assert(discovery.admission?.serviceActive === true, "public admission stop must be active first");
  assert(
    discovery.admission?.serviceEnabled === true,
    "public admission stop must persist across reboot",
  );
  assert(discovery.admission?.edgeFilterInstalled === true, "Proxmox TCP 80/443 stop is absent");
  assert(
    Array.isArray(discovery.vm.disks) && discovery.vm.disks.length >= 2,
    "VM disk inventory is incomplete",
  );
  assert(Array.isArray(discovery.backups), "backup inventory is missing");
  assert(Array.isArray(discovery.guest?.credentials), "credential inventory is missing");
  assert(Array.isArray(discovery.guest?.certificates), "certificate inventory is missing");
  assert(
    Array.isArray(discovery.retainedEvidence) && discovery.retainedEvidence.length > 0,
    "retained evidence inventory is missing",
  );
  assert(DIGEST.test(discovery.guest.release.current), "current release digest is invalid");

  const identity = {
    schemaVersion: 1,
    environment: "public-beta",
    publicOrigin: "https://cloud-test.makodb.com",
    publicIpv4: "130.245.173.11",
    vm: discovery.vm,
    admission: discovery.admission,
    backups: discovery.backups,
    guest: discovery.guest,
    dns: discovery.dns,
    retainedEvidence: discovery.retainedEvidence,
    consequences: [
      "VM deletion makes the single-VM beta unavailable.",
      "Deleting the OS or MAKO_DATA disk is irreversible without a verified retained backup.",
      "Deleting off-VM checkpoints removes the VM-failure-domain recovery path.",
      "Revoked service credentials and certificates cannot be reused after teardown.",
      "The cloud-test.makodb.com A record must be removed or repointed separately by its DNS operator.",
    ],
  };
  const planHash = createHash("sha256").update(canonicalJson(identity)).digest("hex");
  return {
    ...identity,
    generatedAt,
    planHash,
    executionSupported: false,
    requiredTypedConfirmations: {
      revokeCredentials: `REVOKE_PUBLIC_BETA_CREDENTIALS:${planHash}`,
      handleCertificates: `HANDLE_PUBLIC_BETA_CERTIFICATES:${planHash}`,
      handleDns: `HANDLE_PUBLIC_BETA_DNS:${planHash}`,
      deleteBackups: `DELETE_PUBLIC_BETA_BACKUPS:${planHash}`,
      deleteVmAndDisks: `DELETE_PUBLIC_BETA_VM_AND_DISKS:${planHash}`,
    },
    nextAction:
      "Review this plan and use a separately implemented teardown executor; this planner never deletes or revokes anything.",
  };
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
