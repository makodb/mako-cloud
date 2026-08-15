#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import { basename, resolve } from "node:path";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);
const root = resolve(import.meta.dirname, "../..");
const options = parseOptions(process.argv.slice(2));
const pin = JSON.parse(
  await readFile(resolve(root, "infra/proxmox/public-beta/image-pin.json"), "utf8"),
);
const filename = basename(new URL(pin.source).pathname);

assert(pin.distribution === "ubuntu" && pin.release === "24.04", "unexpected guest release");
assert(pin.architecture === "amd64", "unexpected guest architecture");
assert(/^https:\/\//.test(pin.source), "guest image source must use HTTPS");
assert(/^https:\/\//.test(pin.checksumManifest), "checksum manifest must use HTTPS");
assert(/^[0-9a-f]{64}$/.test(pin.sha256), "guest image checksum is invalid");
assert(
  basename(pin.hostPath) === basename(pin.localVolume.split(":", 2)[1]),
  "host image path does not match the Proxmox volume name",
);

const response = await fetch(pin.checksumManifest, { redirect: "error" });
assert(response.ok, `checksum manifest returned HTTP ${response.status}`);
const manifest = await response.text();
const manifestChecksum = manifest
  .split(/\r?\n/)
  .map((line) => line.match(/^([0-9a-f]{64}) [* ](.+)$/))
  .find((match) => match?.[2] === filename)?.[1];
assert(manifestChecksum === pin.sha256, "official manifest does not contain the pinned checksum");

if (options.file) {
  assert((await hashFile(resolve(options.file))) === pin.sha256, "local image checksum mismatch");
}
if (options.remoteHost) {
  const remoteChecksum = (
    await execFile("ssh", ["-o", "BatchMode=yes", options.remoteHost, "sha256sum", pin.hostPath])
  ).stdout.split(/\s+/, 1)[0];
  assert(remoteChecksum === pin.sha256, "cached Proxmox image checksum mismatch");
  const imageInfo = JSON.parse(
    (
      await execFile("ssh", [
        "-o",
        "BatchMode=yes",
        options.remoteHost,
        "qemu-img",
        "info",
        "--output=json",
        pin.hostPath,
      ])
    ).stdout,
  );
  assert(imageInfo.format === "qcow2", "cached guest image is not QCOW2");
  assert(imageInfo["dirty-flag"] === false, "cached guest image has a dirty flag");
  assert(imageInfo["format-specific"]?.data?.corrupt === false, "cached guest image is corrupt");
}

console.log(`verified pinned Ubuntu 24.04 image ${pin.sha256}`);

async function hashFile(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

function parseOptions(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 2) {
    const argument = args[index];
    const value = args[index + 1];
    assert(argument?.startsWith("--") && value !== undefined, "validator options require values");
    result[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
  }
  return result;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
