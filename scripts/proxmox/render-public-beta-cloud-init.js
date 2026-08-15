#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { promisify } from "node:util";

import { assertValidPlan } from "./public-beta-plan-lib.js";

const execFile = promisify(execFileCallback);
const root = resolve(import.meta.dirname, "../..");
const options = parseOptions(process.argv.slice(2));
const planPath = resolve(options.plan ?? "docs/evidence/public-beta-preflight-plan.json");
const sshPublicKeyPath = resolve(options.sshPublicKey);
const output = resolve(options.output);
const plan = assertValidPlan(JSON.parse(await readFile(planPath, "utf8")));
const sshPublicKey = (await readFile(sshPublicKeyPath, "utf8")).trim();
const fingerprint = await publicKeyFingerprint(sshPublicKeyPath);

assert(plan.checks.conflictFree, "cloud-init rendering requires a conflict-free plan");
assert(
  plan.changes.classification !== "destructive",
  "cloud-init rendering refuses a destructive plan",
);
assert(
  fingerprint === plan.security.sshPublicKeyFingerprint,
  "SSH public key does not match the plan",
);
assert(
  /^(?:ssh-ed25519|ecdsa-sha2-|sk-ssh-ed25519@)/.test(sshPublicKey),
  "unsupported SSH public key",
);
assert(!/[\r\n]/.test(sshPublicKey), "SSH public key must occupy one line");

const replacements = {
  "@@SSH_PUBLIC_KEY@@": sshPublicKey,
  "@@PLAN_HASH@@": plan.planHash,
  "@@IPV4_ADDRESS@@": plan.identity.ipv4Address,
  "@@PREFIX_LENGTH@@": String(plan.network.prefixLength),
  "@@GATEWAY@@": plan.network.gateway,
  "@@DNS_SERVERS@@": `[${plan.network.dnsServers.join(", ")}]`,
  "@@MAC_ADDRESS@@": plan.network.macAddress,
};
await mkdir(output, { recursive: true, mode: 0o700 });
for (const [templateName, outputName] of [
  ["user-data", "user-data"],
  ["network-data", "network-config"],
  ["meta-data", "meta-data"],
]) {
  const template = await readFile(
    resolve(root, `infra/proxmox/public-beta/cloud-init/${templateName}.yaml.tmpl`),
    "utf8",
  );
  let rendered = template;
  for (const [placeholder, value] of Object.entries(replacements)) {
    rendered = rendered.replaceAll(placeholder, value);
  }
  assert(!rendered.includes("@@"), `${outputName} has unresolved placeholders`);
  assert(
    !/(?:BEGIN [A-Z ]*PRIVATE KEY|password\s*:|token\s*:|secret\s*:)/i.test(rendered),
    `${outputName} contains secret material`,
  );
  await writeFile(resolve(output, outputName), rendered, { mode: 0o600 });
}
console.log(`rendered secret-free cloud-init seed for plan ${plan.planHash} in ${output}`);

async function publicKeyFingerprint(path) {
  const { stdout } = await execFile("ssh-keygen", ["-l", "-E", "sha256", "-f", path], {
    encoding: "utf8",
    timeout: 10_000,
  });
  return stdout.trim().split(/\s+/)[1];
}

function parseOptions(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 2) {
    const argument = args[index];
    const value = args[index + 1];
    assert(argument?.startsWith("--") && value !== undefined, "renderer options require values");
    result[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
  }
  assert(result.sshPublicKey, "--ssh-public-key is required");
  assert(result.output, "--output is required");
  return result;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
