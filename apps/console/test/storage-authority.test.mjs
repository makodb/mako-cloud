import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import test from "node:test";

const consoleRoot = resolve(import.meta.dirname, "..");

test("console has no authoritative browser database dependency or durable local authority", async () => {
  const packageDocument = JSON.parse(await readFile(join(consoleRoot, "package.json"), "utf8"));
  const dependencies = Object.keys(packageDocument.dependencies ?? {});
  for (const forbidden of ["dexie", "idb", "sql.js", "wa-sqlite", "@sqlite.org/sqlite-wasm"]) {
    assert(!dependencies.includes(forbidden), `forbidden browser database dependency: ${forbidden}`);
  }

  const sources = await sourceFiles(join(consoleRoot, "src"));
  const text = (await Promise.all(sources.map((path) => readFile(path, "utf8")))).join("\n");
  for (const forbidden of [
    /\bindexedDB\b/u,
    /\blocalStorage\b/u,
    /\bDexie\b/u,
    /\bopenDatabase\s*\(/u,
    /\bnew\s+RxDatabase\b/u,
    /\bSQLiteDatabase\b/u,
  ]) {
    assert(!forbidden.test(text), `console source contains browser authority surface: ${forbidden}`);
  }
  assert.match(text, /window\.sessionStorage/u, "developer token must retain tab-scoped storage");
});

async function sourceFiles(directory) {
  const output = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) output.push(...(await sourceFiles(path)));
    else if (entry.isFile() && /\.(?:ts|tsx)$/u.test(entry.name)) output.push(path);
  }
  return output;
}
