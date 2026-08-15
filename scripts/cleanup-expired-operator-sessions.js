#!/usr/bin/env node

import { lstat, readFile, readdir, realpath, unlink } from "node:fs/promises";
import { isAbsolute, join } from "node:path";

const MAX_TOKEN_BYTES = 32 * 1024;
const args = process.argv.slice(2);
let directory;
let apply = false;
for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (argument === "--apply") {
    if (apply) throw new Error("--apply must not be repeated");
    apply = true;
  } else if (argument === "--directory") {
    if (directory !== undefined || index + 1 >= args.length)
      throw new Error("--directory requires one value");
    directory = args[index + 1];
    index += 1;
  } else {
    throw new Error("usage: cleanup-expired-operator-sessions --directory ABSOLUTE [--apply]");
  }
}
if (directory === undefined || !isAbsolute(directory))
  throw new Error("--directory must be an absolute path");

const directoryStat = await lstat(directory);
if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink())
  throw new Error("--directory must be a non-symlink directory");
const resolvedDirectory = await realpath(directory);
if (resolvedDirectory === "/") throw new Error("filesystem root is not an eligible directory");

const now = Math.floor(Date.now() / 1_000);
const eligible = [];
for (const entry of await readdir(resolvedDirectory, { withFileTypes: true })) {
  if (!entry.isFile() || entry.isSymbolicLink() || !entry.name.endsWith(".jwt")) continue;
  const path = join(resolvedDirectory, entry.name);
  const before = await lstat(path);
  if (!before.isFile() || before.isSymbolicLink() || (before.mode & 0o777) !== 0o600) continue;
  if (before.size === 0 || before.size > MAX_TOKEN_BYTES) continue;
  const token = await readFile(path, "utf8");
  const claims = parseExpiredOperatorClaims(token, now);
  if (claims === null) continue;
  eligible.push({ path, before, expiresAtUnixSeconds: claims.exp });
}

const removed = [];
if (apply) {
  for (const candidate of eligible) {
    const current = await lstat(candidate.path);
    if (
      !current.isFile() ||
      current.isSymbolicLink() ||
      current.dev !== candidate.before.dev ||
      current.ino !== candidate.before.ino ||
      current.size !== candidate.before.size ||
      (current.mode & 0o777) !== 0o600
    ) {
      throw new Error("eligible token changed during cleanup; nothing further was removed");
    }
    await unlink(candidate.path);
    removed.push(candidate.path);
  }
}

console.log(
  JSON.stringify(
    {
      schemaVersion: 1,
      mode: apply ? "apply" : "preview",
      directory: resolvedDirectory,
      eligible: eligible.map(({ path, expiresAtUnixSeconds }) => ({ path, expiresAtUnixSeconds })),
      removed,
    },
    null,
    2,
  ),
);

function parseExpiredOperatorClaims(token, nowUnixSeconds) {
  if (token.trim() !== token || token.includes("\n") || token.length > MAX_TOKEN_BYTES) return null;
  const segments = token.split(".");
  if (segments.length !== 3 || segments.some((segment) => segment.length === 0)) return null;
  let claims;
  try {
    claims = JSON.parse(Buffer.from(segments[1], "base64url").toString("utf8"));
  } catch {
    return null;
  }
  if (claims === null || typeof claims !== "object" || Array.isArray(claims)) return null;
  if (!Array.isArray(claims.aud) || !claims.aud.includes("mako-operator")) return null;
  if (!Number.isSafeInteger(claims.exp) || claims.exp < 0 || claims.exp > nowUnixSeconds)
    return null;
  return claims;
}
