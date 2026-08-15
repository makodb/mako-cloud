#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { resolve } from "node:path";
import { promisify } from "node:util";

import { collectConflictEvidence } from "./public-beta-conflicts.js";
import { discoverProxmox } from "./public-beta-discovery.js";
import { buildPlan, PlanBlockedError } from "./public-beta-planner.js";

const execFile = promisify(execFileCallback);
const root = resolve(import.meta.dirname, "../..");
const options = parseOptions(process.argv.slice(2));

try {
  const request = await readJson(
    options.request ?? resolve(root, "infra/proxmox/public-beta/request.json"),
  );
  const imagePin = await readJson(
    options.imagePin ?? resolve(root, "infra/proxmox/public-beta/image-pin.json"),
  );
  const discovery = options.fixture
    ? (await readJson(options.fixture)).discovery
    : await discoverProxmox();
  const conflictEvidence = options.fixture
    ? (await readJson(options.fixture)).conflictEvidence
    : await collectConflictEvidence({
        fqdn: request.identity.fqdn,
        targetAddress: request.identity.ipv4Address,
      });
  const managementCidrs = deriveManagementCidrs(options.managementCidrs);
  const sshPublicKeyFingerprint = await fingerprint(
    options.sshPublicKey ??
      process.env.MAKO_BETA_SSH_PUBLIC_KEY_FILE ??
      resolve(homedir(), ".ssh/id_ed25519.pub"),
  );
  const plan = buildPlan({
    request,
    discovery,
    conflictEvidence,
    imagePin,
    managementCidrs,
    sshPublicKeyFingerprint,
    storageSelections: {
      os: options.osStorage,
      data: options.dataStorage,
      backup: options.backupStorage,
    },
  });
  const serialized = `${JSON.stringify(plan, null, 2)}\n`;
  if (options.output) {
    await writeFile(resolve(options.output), serialized, { flag: "wx", mode: 0o600 });
    console.log(`wrote conflict-free read-only plan ${plan.planHash} to ${options.output}`);
  } else {
    process.stdout.write(serialized);
  }
} catch (error) {
  if (error instanceof PlanBlockedError) {
    console.error("public beta plan blocked before mutation:");
    for (const blocker of error.blockers) console.error(`- ${blocker}`);
    process.exitCode = 2;
  } else {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}

function parseOptions(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (!argument.startsWith("--")) throw new Error(`unexpected argument: ${argument}`);
    const key = argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    const value = args[index + 1];
    if (value === undefined || value.startsWith("--"))
      throw new Error(`${argument} requires a value`);
    result[key] = value;
    index += 1;
  }
  return result;
}

function deriveManagementCidrs(configured) {
  const values = configured ?? process.env.MAKO_BETA_MANAGEMENT_CIDRS;
  if (values)
    return values
      .split(",")
      .map((value) => value.trim())
      .filter(Boolean);
  const source = process.env.SSH_CONNECTION?.split(/\s+/)[0];
  if (source && /^\d{1,3}(?:\.\d{1,3}){3}$/.test(source)) return [`${source}/32`];
  throw new Error(
    "management CIDR is not derivable; pass --management-cidrs or MAKO_BETA_MANAGEMENT_CIDRS",
  );
}

async function fingerprint(path) {
  await readFile(path, "utf8");
  try {
    const { stdout } = await execFile("ssh-keygen", ["-l", "-E", "sha256", "-f", path], {
      encoding: "utf8",
      timeout: 10_000,
    });
    const value = stdout.trim().split(/\s+/)[1];
    if (!value?.startsWith("SHA256:")) throw new Error("unexpected ssh-keygen output");
    return value;
  } catch (error) {
    throw new Error(`cannot fingerprint SSH public key ${path}: ${safeError(error)}`);
  }
}

async function readJson(path) {
  return JSON.parse(await readFile(resolve(path), "utf8"));
}

function safeError(error) {
  return (error instanceof Error ? error.message : String(error))
    .replace(/[\r\n]+/g, " ")
    .slice(0, 300);
}
