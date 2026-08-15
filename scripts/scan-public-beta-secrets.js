#!/usr/bin/env node

import { readdir, readFile } from "node:fs/promises";
import { extname, join, relative, resolve, sep } from "node:path";

const root = resolve(import.meta.dirname, "..");
const roots = [".env.example", ".github", "docs", "infra", "scripts"];
const ignoredDirectories = new Set([
  ".git",
  ".local",
  "dist",
  "node_modules",
  "target",
  "test-results",
]);
const textExtensions = new Set([
  "",
  ".env",
  ".j2",
  ".js",
  ".json",
  ".md",
  ".nft",
  ".service",
  ".sh",
  ".tmpl",
  ".toml",
  ".ts",
  ".yaml",
  ".yml",
]);
const signatures = [
  ["private key block", /-----BEGIN (?:DSA |EC |OPENSSH |PGP |RSA )?PRIVATE KEY-----/u],
  ["GitHub token", /\bgh(?:p|o|u|s|r)_[A-Za-z0-9]{30,}\b/u],
  ["AWS access key", /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/u],
  ["Slack token", /\bxox(?:a|b|p|r|s)-[A-Za-z0-9-]{20,}\b/u],
  ["OpenAI API key", /\bsk-(?:proj-)?[A-Za-z0-9_-]{32,}\b/u],
  ["credential-bearing URL", /https?:\/\/[^\s/:@]+:[^\s/@]+@/u],
  ["literal npm auth token", /^\s*\/\/[^\s]+\/:_authToken=(?!\$\{|<|REDACTED)[^\s]+/mu],
];

const files = [];
for (const entry of roots) await walk(resolve(root, entry), files);

const findings = [];
for (const path of files.sort()) {
  const content = await readFile(path, "utf8");
  for (const [name, pattern] of signatures) {
    if (pattern.test(content)) findings.push(`${repositoryPath(path)}: ${name}`);
  }
}

if (findings.length > 0) {
  throw new Error(`high-confidence secret material found:\n${findings.join("\n")}`);
}
console.log(
  `scanned ${files.length} public-beta source and evidence files; no secret material found`,
);

async function walk(path, files) {
  const entries = await readdir(path, { withFileTypes: true }).catch((error) => {
    if (error?.code !== "ENOTDIR") throw error;
    files.push(path);
    return null;
  });
  if (entries === null) return;
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    if (entry.name.startsWith(".") && entry.name !== ".github") continue;
    if (ignoredDirectories.has(entry.name)) continue;
    const child = join(path, entry.name);
    if (entry.isDirectory()) await walk(child, files);
    else if (entry.isFile() && textExtensions.has(extname(entry.name))) files.push(child);
  }
}

function repositoryPath(path) {
  return relative(root, path).split(sep).join("/");
}
