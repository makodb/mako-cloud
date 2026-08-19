import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

export const repositoryRoot = resolve(import.meta.dirname, "..");
export const betaHost = "mako-admin@130.245.173.11";
export const betaSshArguments = [
  "-o",
  "BatchMode=yes",
  "-o",
  "IdentitiesOnly=yes",
  "-o",
  "StrictHostKeyChecking=yes",
  "-o",
  `UserKnownHostsFile=${resolve(repositoryRoot, ".local/ansible/public-beta-known-hosts")}`,
  betaHost,
];
export const digestPattern = /^[0-9a-f]{64}$/u;

export function ssh(command, { input } = {}) {
  return execFileSync("ssh", [...betaSshArguments, command], {
    cwd: repositoryRoot,
    encoding: "utf8",
    input,
    maxBuffer: 16 * 1024 * 1024,
  }).trim();
}

export function selectedRelease() {
  const release = ssh('basename -- "$(readlink -f /opt/mako/current)"');
  assert(digestPattern.test(release), "guest selected release is not an immutable digest");
  return release;
}

export function assertSelectedRelease(expected) {
  assert(digestPattern.test(expected), "expected release digest is invalid");
  const actual = selectedRelease();
  assert(actual === expected, `guest selected ${actual}, expected ${expected}`);
  return actual;
}

export function qualificationDeveloper() {
  const path = resolve(repositoryRoot, ".local/qualification/public-beta-developer.json");
  const developer = JSON.parse(readFileSync(path, "utf8"));
  assert(developer.schemaVersion === 1, "qualification developer schema is invalid");
  assert(
    typeof developer.developerIdentityId === "string" &&
      /^[A-Za-z0-9_-]{8,128}$/u.test(developer.developerIdentityId),
    "qualification developer identity is invalid",
  );
  assert(
    typeof developer.email === "string" &&
      developer.email.length <= 320 &&
      developer.email.includes("@") &&
      !/[\r\n\0]/u.test(developer.email),
    "qualification developer email is invalid",
  );
  assert(
    typeof developer.displayName === "string" &&
      developer.displayName.length >= 1 &&
      developer.displayName.length <= 200 &&
      !/[\r\n\0]/u.test(developer.displayName),
    "qualification developer display name is invalid",
  );
  assert(
    Number.isSafeInteger(developer.authorizationEpoch) && developer.authorizationEpoch >= 1,
    "qualification developer authorization epoch is invalid",
  );
  // The control plane compares this against the stored identity, so it cannot be
  // defaulted: a wrong value authenticates as a stale session and is rejected.
  assert(
    Number.isSafeInteger(developer.credentialEpoch) && developer.credentialEpoch >= 1,
    "qualification developer credential epoch is missing or invalid",
  );
  return developer;
}

export function runLogged(command, args, logPath, environment = {}) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    env: { ...process.env, ...environment },
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const log = [result.stdout ?? "", result.stderr ?? ""].join("");
  mkdirSync(dirname(resolve(repositoryRoot, logPath)), { recursive: true });
  writeFileSync(resolve(repositoryRoot, logPath), log, { mode: 0o600 });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed; see ${logPath}`);
  }
}

export function parseOptions(args) {
  const options = {};
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index];
    const value = args[index + 1];
    assert(key?.startsWith("--") && value !== undefined, "options require --name value pairs");
    options[key.slice(2).replace(/-([a-z])/gu, (_, letter) => letter.toUpperCase())] = value;
  }
  return options;
}

export function writeJson(path, value) {
  const absolute = resolve(repositoryRoot, path);
  mkdirSync(dirname(absolute), { recursive: true });
  writeFileSync(absolute, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

export function assert(condition, message) {
  if (!condition) throw new Error(message);
}
