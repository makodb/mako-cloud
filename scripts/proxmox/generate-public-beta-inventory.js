#!/usr/bin/env node

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

import { assertValidPlan } from "./public-beta-plan-lib.js";

const options = parseOptions(process.argv.slice(2));
const plan = assertValidPlan(
  JSON.parse(
    await readFile(
      resolve(options.plan ?? "docs/evidence/public-beta-preflight-plan.json"),
      "utf8",
    ),
  ),
);
const output = resolve(options.output ?? ".local/ansible/public-beta.ini");
const inventory = [
  "[public_beta]",
  `${plan.identity.vmName} ansible_host=${plan.identity.ipv4Address} ansible_user=mako-admin`,
  "",
  "[public_beta:vars]",
  `mako_inventory_plan_hash=${plan.planHash}`,
  "",
].join("\n");
await mkdir(dirname(output), { recursive: true, mode: 0o700 });
await writeFile(output, inventory, { mode: 0o600 });
console.log(`wrote secret-free Ansible inventory for plan ${plan.planHash} to ${output}`);

function parseOptions(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 2) {
    const argument = args[index];
    const value = args[index + 1];
    if (!argument?.startsWith("--") || value === undefined)
      throw new Error("options require values");
    result[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
  }
  return result;
}
