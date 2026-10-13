import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { downloadMultipart } from "../dist/commands/export-download.js";

const digest = (value) => `sha256:${createHash("sha256").update(value).digest("hex")}`;
const chunks = ['{"id":"a"}\n', '{"id":"b"}\n', '{"id":"c"}\n'];
function fixture() {
  const tenant = { projectId: "prj_abcdefgh", environmentId: "env_abcdefgh" };
  return { jobId: "djob_abcdefgh", tenant, collectionId: "todos", kind: "export", state: "succeeded",
    manifest: { formatVersion: 2, tenant, collectionId: "todos", schemaVersion: 1, snapshot: "snapshot", rowCount: 3,
      byteCount: chunks.join("").length, digest: digest(chunks.join("")), finalizedAtUnixSeconds: 1,
      parts: chunks.map((value) => ({ digest: digest(value), byteCount: value.length, rowCount: 1 })) } };
}
const response = (index, body = chunks[index]) => new Response(body, { headers: { Digest: `sha-256=${digest(chunks[index]).slice(7)}` } });
async function destination(t) {
  const dir = await mkdtemp(join(tmpdir(), "mako-export-test-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return join(dir, "export.jsonl");
}

test("interrupted download resumes verified prefix and atomically replaces output", async (t) => {
  const output = await destination(t);
  await writeFile(output, "original");
  await assert.rejects(downloadMultipart(fixture(), output, async (index) => {
    if (index === 2) throw new Error("connection lost");
    return response(index);
  }), /connection lost/u);
  assert.equal(await readFile(output, "utf8"), "original");
  assert.equal(await readFile(`${output}.part`, "utf8"), chunks.slice(0, 2).join(""));
  assert.equal((await stat(`${output}.part`)).mode & 0o777, 0o600);
  const requested = [];
  const result = await downloadMultipart(fixture(), output, async (index) => { requested.push(index); return response(index); });
  assert.deepEqual(requested, [2]);
  assert.equal(result.digest, fixture().manifest.digest);
  assert.equal(await readFile(output, "utf8"), chunks.join(""));
  await assert.rejects(stat(`${output}.mako-export.json`), { code: "ENOENT" });
});

test("corrupt cached prefix redownloads its suffix", async (t) => {
  const output = await destination(t);
  await assert.rejects(downloadMultipart(fixture(), output, async (index) => {
    if (index === 2) throw new Error("interrupted");
    return response(index);
  }));
  await writeFile(`${output}.part`, `x${chunks.slice(0, 2).join("").slice(1)}`);
  const requested = [];
  await downloadMultipart(fixture(), output, async (index) => { requested.push(index); return response(index); });
  assert.deepEqual(requested, [0, 1, 2]);
});

test("resume refuses another job and concurrent writer", async (t) => {
  const output = await destination(t);
  await assert.rejects(downloadMultipart(fixture(), output, async () => { throw new Error("interrupted"); }));
  await assert.rejects(downloadMultipart({ ...fixture(), jobId: "different" }, output, async () => response(0)), /different export/u);
  await writeFile(`${output}.mako-export.lock`, String(process.pid), { mode: 0o600 });
  await assert.rejects(downloadMultipart(fixture(), output, async () => response(0)), /another process/u);
});

test("bad part, oversized body, and wrong final digest never publish", async (t) => {
  for (const kind of ["corrupt", "oversized", "final"]) {
    const output = await destination(t);
    const job = fixture();
    if (kind === "final") job.manifest.digest = digest("wrong");
    await assert.rejects(downloadMultipart(job, output, async (index) => {
      if (index === 1 && kind !== "final") return response(index, kind === "corrupt" ? "x".repeat(chunks[index].length) : `${chunks[index]}extra`);
      return response(index);
    }), /integrity|exceeds/u);
    await assert.rejects(stat(output), { code: "ENOENT" });
    if (kind !== "final") assert.equal((await stat(`${output}.part`)).size, chunks[0].length);
  }
});
