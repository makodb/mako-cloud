#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const ansibleLint = existsSync(resolve(root, ".local/ansible-venv/bin/ansible-lint"))
  ? resolve(root, ".local/ansible-venv/bin/ansible-lint")
  : "ansible-lint";

console.log(
  "SAFE LOCAL VALIDATION: offline only; no SSH, Proxmox API, DNS mutation, or admission change",
);
for (const [label, command, args, environment = {}] of [
  ["provisioning collision fixtures", "npm", ["run", "test:proxmox-plan"]],
  ["idempotent apply fixtures", "npm", ["run", "test:proxmox-apply"]],
  ["non-executing teardown fixtures", "npm", ["run", "test:proxmox-teardown"]],
  [
    "cloud-init, firewall, systemd, manifest, and evidence",
    "npm",
    ["run", "validate:public-beta-infrastructure"],
  ],
  ["Caddy route exposure", "npm", ["run", "validate:public-beta-caddy"]],
  ["rootless dependency isolation", "npm", ["run", "validate:public-beta-containers"]],
  ["native service composition", "npm", ["run", "validate:public-beta-services"]],
  ["observability assets", "npm", ["run", "validate:observability"]],
  ["high-confidence secret scan", "npm", ["run", "scan:public-beta-secrets"]],
  [
    "Ansible production lint",
    ansibleLint,
    ["infra/ansible/playbooks/public-beta.yml"],
    { ANSIBLE_CONFIG: "infra/ansible/ansible.cfg" },
  ],
]) {
  console.log(`\n[offline] ${label}`);
  const result = spawnSync(command, args, {
    cwd: root,
    env: { ...process.env, ...environment },
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

console.log(`
OFFLINE VALIDATION PASSED
The following phases remain intentionally separate and were not run:
- read-only live discovery: npm run plan:public-beta
- hash-bound external mutation: npm run proxmox:public-beta -- --apply-plan <path> --confirm <typed-value>
- public preview: requires exact-release evidence, all non-waivable safeguards, and a persistent manually revocable digest-bound approval
- qualified beta: remains blocked until every release threshold and the separate beta approval pass
- destructive teardown: unsupported by ordinary convergence; the planner is read-only and executionSupported=false`);
