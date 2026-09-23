import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { dirname, extname, join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repositoryDirectory = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const commandNames = [
  "activity",
  "allowed-origins",
  "auth",
  "auth-settings",
  "backups",
  "collections",
  "data",
  "domains",
  "email-templates",
  "envs",
  "explorer",
  "functions",
  "indexes",
  "keys",
  "logs",
  "observability",
  "policies",
  "projects",
  "schedules",
  "storage",
  "sync",
  "teams",
  "usage",
  "users",
  "webhooks",
  "workspace",
].join("|");
const stalePatterns = [
  new RegExp(`\\bmako (?=(?:${commandNames})(?:\\b|$))`, "gu"),
  /node_modules\/\.bin\/mako(?!-cloud)/gu,
  /`mako` (?:CLI|binary|command|exits)/gu,
  /once linked:\s*mako(?:\s|$)/gu,
];
const scanRoots = [
  "crates",
  "docs",
  "examples",
  "openspec/specs",
  "packages/cli/src",
  "scripts",
  "services",
];
const textExtensions = new Set([".js", ".json", ".md", ".mjs", ".rs", ".ts", ".tsx"]);
const skippedDirectories = new Set([".git", ".local", "dist", "node_modules", "target"]);

async function textFiles(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (!skippedDirectories.has(entry.name)) files.push(...(await textFiles(path)));
    } else if (textExtensions.has(extname(entry.name))) {
      files.push(path);
    }
  }
  return files;
}

test("the package installs only the mako-cloud executable", async () => {
  const manifest = JSON.parse(
    await readFile(join(repositoryDirectory, "packages/cli/package.json"), "utf8"),
  );
  assert.deepEqual(manifest.bin, { "mako-cloud": "dist/main.js" });
});

test("current sources and documentation do not use the old developer command", async () => {
  const files = (
    await Promise.all(scanRoots.map((root) => textFiles(join(repositoryDirectory, root))))
  ).flat();
  const stale = [];
  for (const path of files) {
    const contents = await readFile(path, "utf8");
    for (const pattern of stalePatterns) {
      for (const match of contents.matchAll(pattern)) {
        const line = contents.slice(0, match.index).split("\n").length;
        stale.push(`${relative(repositoryDirectory, path)}:${line}: ${match[0]}`);
      }
    }
  }
  assert.deepEqual(stale, [], `old developer command references:\n${stale.join("\n")}`);
});
