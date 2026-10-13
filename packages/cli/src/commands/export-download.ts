import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { Readable } from "node:stream";
import type { ReadableStream as WebReadableStream } from "node:stream/web";
import { lstat, open, rename, unlink, writeFile } from "node:fs/promises";
import type { DataJob } from "@mako-cloud/management-sdk";

function fail(message: string): never {
  throw new Error(`export download: ${message}`);
}

function missing(error: unknown): boolean {
  return (error as NodeJS.ErrnoException).code === "ENOENT";
}

/** A protected partial file retains only verified parts after a failed transfer. */
export async function downloadMultipart(
  job: DataJob,
  output: string,
  requestPart: (index: number) => Promise<Response>,
): Promise<{ bytes: number; digest: string }> {
  const manifest = job.manifest;
  const parts = manifest?.parts;
  if (
    job.kind !== "export" ||
    job.state !== "succeeded" ||
    manifest?.formatVersion !== 2 ||
    !parts?.length ||
    parts.length > 4096 ||
    !/^sha256:[a-f0-9]{64}$/u.test(manifest.digest) ||
    manifest.tenant.projectId !== job.tenant.projectId ||
    manifest.tenant.environmentId !== job.tenant.environmentId ||
    manifest.collectionId !== job.collectionId
  ) {
    fail("invalid multipart manifest");
  }
  let totalBytes = 0;
  let totalRows = 0;
  for (const part of parts) {
    if (
      !/^sha256:[a-f0-9]{64}$/u.test(part.digest) ||
      !Number.isSafeInteger(part.byteCount) ||
      part.byteCount < 1 ||
      part.byteCount > 20 * 1024 * 1024 ||
      !Number.isSafeInteger(part.rowCount) ||
      part.rowCount < 0
    )
      fail("invalid part metadata");
    totalBytes += part.byteCount;
    totalRows += part.rowCount;
  }
  if (totalBytes !== manifest.byteCount || totalRows !== manifest.rowCount)
    fail("inconsistent manifest totals");
  const partial = `${output}.part`;
  const metadata = `${output}.mako-export.json`;
  const lockPath = `${output}.mako-export.lock`;
  // Exclusive ownership prevents two CLI invocations corrupting one download.
  // A crash leaves a PID, which can be recovered only after that process exits.
  let lock: Awaited<ReturnType<typeof open>>;
  try {
    lock = await open(lockPath, "wx", 0o600);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    const info = await lstat(lockPath);
    if (!info.isFile() || info.isSymbolicLink()) fail("unsafe download lock");
    const existing = await open(lockPath, "r");
    const pid = Number(await existing.readFile("utf8"));
    await existing.close();
    if (!Number.isSafeInteger(pid) || pid < 1) fail(`incomplete lock; inspect ${lockPath}`);
    try {
      process.kill(pid, 0);
      fail("another process owns this download");
    } catch (cause) {
      if ((cause as NodeJS.ErrnoException).code !== "ESRCH") throw cause;
    }
    await unlink(lockPath);
    lock = await open(lockPath, "wx", 0o600);
  }
  await lock.writeFile(String(process.pid));
  try {
    const identity = JSON.stringify({
      version: 1,
      jobId: job.jobId,
      tenant: job.tenant,
      collectionId: job.collectionId,
      manifest,
    });
    let hasMetadata = false;
    try {
      const info = await lstat(metadata);
      if (!info.isFile() || info.isSymbolicLink() || (info.mode & 0o077) !== 0)
        fail("unsafe resume metadata");
      const file = await open(metadata, "r");
      try {
        if ((await file.readFile("utf8")) !== identity)
          fail("partial file belongs to a different export");
      } finally {
        await file.close();
      }
      hasMetadata = true;
    } catch (error) {
      if (!missing(error)) throw error;
    }
    let size = 0;
    try {
      const info = await lstat(partial);
      if (!hasMetadata) fail("partial file has no matching resume metadata");
      if (!info.isFile() || info.isSymbolicLink() || (info.mode & 0o077) !== 0)
        fail("unsafe partial file");
      size = info.size;
    } catch (error) {
      if (!missing(error)) throw error;
    }
    if (!hasMetadata) await writeFile(metadata, identity, { mode: 0o600, flag: "wx" });
    const file = await open(partial, size > 0 ? "r+" : "a+", 0o600);
    try {
      const whole = createHash("sha256");
      let offset = 0;
      for (const [index, part] of parts.entries()) {
        let cached = false;
        if (size >= offset + part.byteCount) {
          const hash = createHash("sha256");
          for await (const chunk of createReadStream(partial, {
            start: offset,
            end: offset + part.byteCount - 1,
          }))
            hash.update(chunk);
          cached = `sha256:${hash.digest("hex")}` === part.digest;
        }
        if (!cached) {
          await file.truncate(offset);
          size = offset;
          let received = 0;
          const hash = createHash("sha256");
          try {
            const response = await requestPart(index);
            if (!response.ok || !response.body)
              fail(`part ${index} returned HTTP ${response.status}`);
            if (response.headers.get("Digest") !== `sha-256=${part.digest.slice(7)}`)
              fail(`part ${index} has an unexpected Digest header`);
            for await (const value of Readable.fromWeb(
              response.body as WebReadableStream<Uint8Array>,
            )) {
              const chunk = Buffer.from(value);
              if (received + chunk.length > part.byteCount)
                fail(`part ${index} exceeds its declared size`);
              hash.update(chunk);
              let written = 0;
              while (written < chunk.length) {
                const result = await file.write(
                  chunk,
                  written,
                  chunk.length - written,
                  offset + received + written,
                );
                if (result.bytesWritten === 0) fail("unable to write partial file");
                written += result.bytesWritten;
              }
              received += chunk.length;
            }
            if (received !== part.byteCount || `sha256:${hash.digest("hex")}` !== part.digest)
              fail(`part ${index} failed integrity verification`);
            await file.sync();
          } catch (error) {
            await file.truncate(offset);
            throw error;
          }
        }
        // Hash the verified on-disk bytes, including previously completed parts.
        let rows = 0;
        for await (const chunk of createReadStream(partial, {
          start: offset,
          end: offset + part.byteCount - 1,
        })) {
          whole.update(chunk);
          for (const byte of chunk as Buffer) if (byte === 10) rows++;
        }
        if (rows !== part.rowCount) fail(`part ${index} has an unexpected row count`);
        offset += part.byteCount;
      }
      await file.truncate(totalBytes);
      const digest = `sha256:${whole.digest("hex")}`;
      if (digest !== manifest.digest) fail("complete artifact failed integrity verification");
      await file.sync();
      await file.close();
      await rename(partial, output);
      await unlink(metadata);
      return { bytes: totalBytes, digest };
    } finally {
      await file.close();
    }
  } finally {
    await lock.close();
    await unlink(lockPath);
  }
}
