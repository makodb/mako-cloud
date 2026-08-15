#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { promisify } from "node:util";

import {
  ApplyBlockedError,
  assertLivePreconditions,
  nextProvisioningAction,
  sanitizedVmEvidence,
} from "./public-beta-apply-lib.js";
import { collectConflictEvidence } from "./public-beta-conflicts.js";
import { discoverProxmox } from "./public-beta-discovery.js";
import { assertValidPlan } from "./public-beta-plan-lib.js";

const execFile = promisify(execFileCallback);
const options = parseOptions(process.argv.slice(2));
const planPath = resolve(options.plan ?? "docs/evidence/public-beta-preflight-plan.json");
const cloudInitDir = resolve(options.cloudInitDir ?? ".local/cloud-init");
const imagePinPath = resolve(options.imagePin ?? "infra/proxmox/public-beta/image-pin.json");
const sshHost = options.sshHost ?? process.env.MAKO_PROXMOX_SSH ?? "root@localhost";

try {
  const plan = assertValidPlan(await readJson(planPath));
  const imagePin = await readJson(imagePinPath);
  assertPlanImage(plan, imagePin);
  const seed = await buildSeed(plan, cloudInitDir, options.seedOutput);
  const seedVolume = seedVolumeFor(plan, imagePin, seed.fileName);
  const remoteSeedPath = `/var/lib/vz/template/iso/${seed.fileName}`;
  const live = await liveState(plan, sshHost);
  assertLivePreconditions(plan, live.discovery, live.evidence);
  const remoteSeed = await inspectRemoteSeed(sshHost, remoteSeedPath, seed.sha256);
  const current = await inspectVm(sshHost, plan);

  if (options.inspect) {
    const output = {
      mode: "inspect",
      planHash: plan.planHash,
      seed: { volume: seedVolume, sha256: seed.sha256, state: remoteSeed },
      vm:
        current === null
          ? null
          : sanitizedVmEvidence(plan, current, remoteSeed === "matching" ? seed.sha256 : null),
    };
    process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
  } else if (!options.apply) {
    const action = nextProvisioningAction({
      plan,
      current,
      imagePath: imagePin.hostPath,
      seedVolume,
      destructiveConfirmationValue: options.confirmDestructive,
    });
    const output = {
      mode: "dry-run",
      planHash: plan.planHash,
      livePreflight: "passed",
      remoteSeed,
      nextAction: remoteSeed === "absent" ? "upload-cloud-init-seed" : action.id,
      command:
        remoteSeed === "absent" || action.command === null ? null : displayCommand(action.command),
      note:
        remoteSeed === "absent"
          ? "VM convergence begins only after the hash-bound seed is installed and reverified."
          : "Apply re-inspects live state after every action and stops on drift.",
    };
    process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
  } else {
    assert(
      options.confirmPlan === plan.planHash,
      `--apply requires --confirm-plan ${plan.planHash}`,
    );
    const evidence = await converge({
      plan,
      imagePin,
      seed,
      seedVolume,
      remoteSeedPath,
      sshHost,
      destructiveConfirmationValue: options.confirmDestructive,
    });
    const evidencePath = resolve(
      options.evidence ?? "docs/evidence/public-beta-provisioning-result.json",
    );
    await writeFileExclusiveOrMatching(evidencePath, evidence);
    console.log(`VM ${plan.proxmox.vmId} converged for plan ${plan.planHash}`);
    console.log(`sanitized provisioning evidence: ${evidencePath}`);
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = error instanceof ApplyBlockedError ? 2 : 1;
}

async function converge({
  plan,
  imagePin,
  seed,
  seedVolume,
  remoteSeedPath,
  sshHost,
  destructiveConfirmationValue,
}) {
  const applied = [];
  let remoteSeed = await inspectRemoteSeed(sshHost, remoteSeedPath, seed.sha256);
  if (remoteSeed === "absent") {
    await uploadSeed(sshHost, seed.path, remoteSeedPath, seed.sha256);
    remoteSeed = await inspectRemoteSeed(sshHost, remoteSeedPath, seed.sha256);
    assert(remoteSeed === "matching", "remote cloud-init seed did not verify after upload");
    applied.push({ id: "upload-cloud-init-seed", result: "completed" });
  }
  for (let iteration = 0; iteration < 20; iteration += 1) {
    const live = await liveState(plan, sshHost);
    assertLivePreconditions(plan, live.discovery, live.evidence);
    const current = await inspectVm(sshHost, plan);
    const action = nextProvisioningAction({
      plan,
      current,
      imagePath: imagePin.hostPath,
      seedVolume,
      destructiveConfirmationValue,
    });
    if (!action.mutating) {
      const finalState = await inspectVm(sshHost, plan);
      assert(finalState !== null, "VM disappeared during final inspection");
      return {
        schemaVersion: 1,
        appliedAt: new Date().toISOString(),
        planHash: plan.planHash,
        mode: "external-apply",
        sshTarget: sshHost,
        actions: applied,
        desiredState: sanitizedVmEvidence(plan, finalState, seed.sha256),
      };
    }
    const result = await remoteCommand(sshHost, action.command, 15 * 60_000);
    applied.push({
      id: action.id,
      command: displayCommand(action.command),
      result: "completed",
      output: sanitizeOutput(result.stdout),
    });
  }
  throw new Error("provisioning did not converge within 20 idempotent actions");
}

async function liveState(plan, sshHost) {
  const previous = process.env.MAKO_PROXMOX_SSH;
  process.env.MAKO_PROXMOX_SSH = sshHost;
  try {
    const [discovery, evidence] = await Promise.all([
      discoverProxmox({ localNode: plan.proxmox.node }),
      collectConflictEvidence({
        fqdn: plan.identity.fqdn,
        targetAddress: plan.identity.ipv4Address,
      }),
    ]);
    return { discovery, evidence };
  } finally {
    if (previous === undefined) delete process.env.MAKO_PROXMOX_SSH;
    else process.env.MAKO_PROXMOX_SSH = previous;
  }
}

async function inspectVm(sshHost, plan) {
  const { node, vmId } = plan.proxmox;
  try {
    const [config, status] = await Promise.all([
      remoteJson(sshHost, [
        "pvesh",
        "get",
        `/nodes/${node}/qemu/${vmId}/config`,
        "--output-format",
        "json",
      ]),
      remoteJson(sshHost, [
        "pvesh",
        "get",
        `/nodes/${node}/qemu/${vmId}/status/current`,
        "--output-format",
        "json",
      ]),
    ]);
    const storageIds = [...new Set([plan.storage.os.id, plan.storage.data.id])];
    const contents = (
      await Promise.all(
        storageIds.map((storageId) =>
          remoteJson(sshHost, [
            "pvesh",
            "get",
            `/nodes/${node}/storage/${storageId}/content`,
            "--vmid",
            String(vmId),
            "--output-format",
            "json",
          ]),
        ),
      )
    ).flat();
    enrichDiskSizes(config, contents);
    return { type: "qemu", config, status: status.status ?? "unknown" };
  } catch (error) {
    if (/does not exist|no such VM|not found/i.test(String(error))) return null;
    throw error;
  }
}

function enrichDiskSizes(config, contents) {
  const sizes = new Map(contents.map((entry) => [entry.volid, Number(entry.size)]));
  for (const bus of ["scsi0", "scsi1"]) {
    if (config[bus] === undefined || String(config[bus]).includes(",size=")) continue;
    const volume = String(config[bus]).split(",", 1)[0];
    const bytes = sizes.get(volume);
    assert(Number.isSafeInteger(bytes) && bytes > 0, `Proxmox did not report a size for ${volume}`);
    config[bus] = `${config[bus]},size=${formatGiB(bytes)}G`;
  }
}

function formatGiB(bytes) {
  const value = bytes / 1024 ** 3;
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(6)));
}

async function buildSeed(plan, cloudInitDir, configuredOutput) {
  const required = ["user-data", "meta-data", "network-config"];
  const contents = Object.fromEntries(
    await Promise.all(
      required.map(async (name) => [name, await readFile(resolve(cloudInitDir, name), "utf8")]),
    ),
  );
  for (const [name, content] of Object.entries(contents)) {
    if (name === "network-config") {
      assert(
        content.includes(`${plan.identity.ipv4Address}/${plan.network.prefixLength}`) &&
          content.includes(plan.network.gateway) &&
          content.toLowerCase().includes(plan.network.macAddress.toLowerCase()) &&
          plan.network.dnsServers.every((server) => content.includes(server)),
        "network-config differs from the hash-bound network values",
      );
    } else {
      assert(content.includes(plan.planHash), `${name} is not bound to plan ${plan.planHash}`);
    }
    assert(!content.includes("@@"), `${name} contains an unresolved placeholder`);
    assert(
      !/(?:BEGIN [A-Z ]*PRIVATE KEY|(^|\n)\s*(?:password|token|secret)\s*:)/i.test(content),
      `${name} contains secret-like material`,
    );
  }
  const bundleSha256 = createHash("sha256")
    .update(required.map((name) => `${name}\0${contents[name]}\0`).join(""))
    .digest("hex");
  const fileName = `mako-cloud-public-beta-seed-${plan.planHash}-${bundleSha256.slice(0, 16)}.iso`;
  const output = resolve(configuredOutput ?? resolve(cloudInitDir, fileName));
  try {
    const existing = await readFile(output);
    return {
      path: output,
      fileName,
      bundleSha256,
      sha256: createHash("sha256").update(existing).digest("hex"),
    };
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const temporaryDirectory = await mkdtemp(resolve(tmpdir(), "mako-cloud-seed-"));
  const temporaryOutput = resolve(temporaryDirectory, fileName);
  try {
    await execFile(
      "genisoimage",
      [
        "-quiet",
        "-output",
        temporaryOutput,
        "-volid",
        "cidata",
        "-joliet",
        "-rock",
        "-graft-points",
        `user-data=${resolve(cloudInitDir, "user-data")}`,
        `meta-data=${resolve(cloudInitDir, "meta-data")}`,
        `network-config=${resolve(cloudInitDir, "network-config")}`,
      ],
      { encoding: "utf8", timeout: 30_000 },
    );
    const bytes = await readFile(temporaryOutput);
    await writeFile(output, bytes, { mode: 0o600 });
    return {
      path: output,
      fileName,
      bundleSha256,
      sha256: createHash("sha256").update(bytes).digest("hex"),
    };
  } finally {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
}

async function inspectRemoteSeed(sshHost, path, expectedSha256) {
  try {
    const result = await remoteCommand(sshHost, ["sha256sum", path]);
    const actual = result.stdout.trim().split(/\s+/, 1)[0];
    if (actual !== expectedSha256) {
      throw new ApplyBlockedError(`remote seed ${path} exists with unexpected SHA-256 ${actual}`);
    }
    return "matching";
  } catch (error) {
    if (/No such file or directory/i.test(String(error))) return "absent";
    throw error;
  }
}

async function uploadSeed(sshHost, localPath, remotePath, expectedSha256) {
  const partial = `${remotePath}.partial`;
  await execFile(
    "scp",
    ["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", localPath, `${sshHost}:${partial}`],
    { encoding: "utf8", timeout: 5 * 60_000 },
  );
  try {
    const result = await remoteCommand(sshHost, ["sha256sum", partial]);
    const actual = result.stdout.trim().split(/\s+/, 1)[0];
    assert(actual === expectedSha256, "uploaded cloud-init seed checksum mismatch");
    await remoteCommand(sshHost, ["mv", "--no-clobber", partial, remotePath]);
  } catch (error) {
    await remoteCommand(sshHost, ["rm", "-f", partial]).catch(() => {});
    throw error;
  }
}

async function remoteJson(sshHost, command) {
  const result = await remoteCommand(sshHost, command);
  return JSON.parse(result.stdout);
}

async function remoteCommand(sshHost, command, timeout = 30_000) {
  assertRemoteCommand(command);
  try {
    return await execFile(
      "ssh",
      ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", sshHost, shellJoin(command)],
      { encoding: "utf8", maxBuffer: 8 * 1024 * 1024, timeout },
    );
  } catch (error) {
    const message = [error?.message, error?.stderr, error?.stdout]
      .filter(Boolean)
      .join(" ")
      .replace(/[\r\n]+/g, " ")
      .slice(0, 1000);
    throw new Error(`remote command failed (${displayCommand(command)}): ${message}`);
  }
}

function seedVolumeFor(plan, imagePin, fileName) {
  const storage = String(imagePin.localVolume).split(":", 1)[0];
  assert(plan.storage.backup.id === storage, "seed ISO storage is not bound to the plan");
  assert(plan.storage.backup.content.includes("iso"), "seed ISO storage lacks ISO content");
  return `${storage}:iso/${fileName}`;
}

function assertPlanImage(plan, imagePin) {
  assert(plan.image.sha256 === imagePin.sha256, "image pin differs from the plan");
  assert(/^\/[a-zA-Z0-9_./-]+$/.test(imagePin.hostPath), "image host path is unsafe");
  assert(
    /^[a-zA-Z0-9_.-]+:iso\/[a-zA-Z0-9_.-]+$/.test(imagePin.localVolume),
    "image volume is unsafe",
  );
}

async function writeFileExclusiveOrMatching(path, value) {
  const serialized = `${JSON.stringify(value, null, 2)}\n`;
  try {
    await writeFile(path, serialized, { flag: "wx", mode: 0o600 });
  } catch (error) {
    if (error?.code !== "EEXIST") throw error;
    const current = JSON.parse(await readFile(path, "utf8"));
    if (
      current.planHash !== value.planHash ||
      current.desiredState?.configDigest !== value.desiredState?.configDigest
    ) {
      throw new Error(`evidence path ${path} already contains a different result`);
    }
  }
}

function parseOptions(args) {
  const result = {};
  const flags = new Set(["--apply", "--inspect", "--dry-run"]);
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    assert(argument.startsWith("--"), `unexpected argument ${argument}`);
    const key = argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    if (flags.has(argument)) result[key] = true;
    else {
      const value = args[index + 1];
      assert(value !== undefined && !value.startsWith("--"), `${argument} requires a value`);
      result[key] = value;
      index += 1;
    }
  }
  assert(!(result.apply && result.inspect), "--apply and --inspect are mutually exclusive");
  return result;
}

function assertRemoteCommand(command) {
  assert(Array.isArray(command) && command.length > 0, "remote command is empty");
  assert(
    ["pvesh", "qm", "sha256sum", "mv", "rm"].includes(command[0]),
    "remote command is not allowlisted",
  );
  for (const argument of command) {
    assert(!/[\0\r\n]/.test(argument), "remote command contains an unsafe character");
  }
}

function displayCommand(command) {
  return command
    .map((argument) =>
      /^[a-zA-Z0-9_./:=,+;-]+$/.test(argument) ? argument : JSON.stringify(argument),
    )
    .join(" ");
}

function shellJoin(command) {
  return command.map((argument) => `'${argument.replaceAll("'", `'"'"'`)}'`).join(" ");
}

function sanitizeOutput(value) {
  return String(value)
    .split(/\r?\n/)
    .filter(
      (line) =>
        !/(?:iscsiadm:|Could not login to|Could not log into|command '\/usr\/bin\/iscsiadm)/i.test(
          line,
        ),
    )
    .join(" ")
    .replace(/[\r\n]+/g, " ")
    .replace(/(password|token|secret|authorization)\s*[=:]\s*\S+/gi, "$1=<redacted>")
    .trim()
    .slice(0, 1000);
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
