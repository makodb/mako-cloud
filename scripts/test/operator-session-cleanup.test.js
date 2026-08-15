import assert from "node:assert/strict";
import { chmod, mkdtemp, readFile, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";
import { execFile } from "node:child_process";

const execute = promisify(execFile);
const script = resolve(import.meta.dirname, "../cleanup-expired-operator-sessions.js");

function token(audience, expiry) {
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${encode({ alg: "EdDSA" })}.${encode({ aud: audience, exp: expiry })}.signature`;
}

test("cleanup removes only explicitly scoped expired operator JWT files", async () => {
  const directory = await mkdtemp(join(tmpdir(), "mako-operator-cleanup-"));
  const expired = join(directory, "expired.jwt");
  const current = join(directory, "current.jwt");
  const otherAudience = join(directory, "developer.jwt");
  const deploymentSecret = join(directory, "internal-auth");
  const looseMode = join(directory, "loose.jwt");
  const link = join(directory, "linked.jwt");
  await Promise.all([
    writeFile(expired, token(["mako-operator"], 1), { mode: 0o600 }),
    writeFile(current, token(["mako-operator"], 4_102_444_800), { mode: 0o600 }),
    writeFile(otherAudience, token(["mako-management"], 1), { mode: 0o600 }),
    writeFile(deploymentSecret, "deployment-secret", { mode: 0o600 }),
    writeFile(looseMode, token(["mako-operator"], 1), { mode: 0o600 }),
  ]);
  await chmod(looseMode, 0o644);
  await symlink(expired, link);

  const preview = JSON.parse(
    (await execute(process.execPath, [script, "--directory", directory])).stdout,
  );
  assert.deepEqual(preview.removed, []);
  assert.deepEqual(
    preview.eligible.map((value) => value.path),
    [expired],
  );
  assert.equal(await readFile(expired, "utf8"), token(["mako-operator"], 1));

  const applied = JSON.parse(
    (await execute(process.execPath, [script, "--directory", directory, "--apply"])).stdout,
  );
  assert.deepEqual(applied.removed, [expired]);
  await assert.rejects(readFile(expired, "utf8"));
  assert.equal(await readFile(current, "utf8"), token(["mako-operator"], 4_102_444_800));
  assert.equal(await readFile(otherAudience, "utf8"), token(["mako-management"], 1));
  assert.equal(await readFile(deploymentSecret, "utf8"), "deployment-secret");
  assert.equal(await readFile(looseMode, "utf8"), token(["mako-operator"], 1));
});
