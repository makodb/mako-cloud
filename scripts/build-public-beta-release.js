#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFile, spawn } from "node:child_process";
import {
  chmod,
  copyFile,
  lstat,
  mkdir,
  readFile,
  readdir,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";

import { assertValidPlan, canonicalJson } from "./proxmox/public-beta-plan-lib.js";

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(import.meta.dirname, "..");
const options = parseOptions(process.argv.slice(2));
const planPath = resolve(
  repositoryRoot,
  options.plan ?? "docs/evidence/public-beta-preflight-plan.json",
);
const candidateRoot = resolve(repositoryRoot, options.output ?? ".local/public-beta-candidates");
const evidencePath = resolve(
  repositoryRoot,
  options.evidence ?? "docs/evidence/public-beta-release-manifest.json",
);
const deploymentSelectorPath = "infra/ansible/group_vars/public_beta.yml";
const plan = assertValidPlan(JSON.parse(await readFile(planPath, "utf8")));

const source = await describeSourceFiles(await collectSourceFiles());
const dependencies = await describeNamedFiles(["Cargo.lock", "package-lock.json"]);
const runtimeValues = await collectRuntimeValues();
const runtime = {
  digest: sha256(canonicalJson(runtimeValues)),
  values: runtimeValues,
};
const artifacts = await collectArtifacts();
const artifactSet = {
  digest: digestEntries(artifacts),
  files: artifacts,
};
const identity = {
  schemaVersion: 1,
  storageCompatibility: {
    controlSqlite: [1],
    tenantRocksDb: [1],
  },
  planHash: plan.planHash,
  source,
  runtime,
  dependencies,
  artifacts: artifactSet,
};
const releaseDigest = sha256(canonicalJson(identity));
const relativeCandidatePath = `.local/public-beta-candidates/${releaseDigest}`;
const manifest = {
  ...identity,
  releaseDigest,
  generatedAt: new Date().toISOString(),
  candidateLayout: {
    root: relativeCandidatePath,
    manifest: "manifest.json",
    binaries: "bin",
    console: "console",
  },
};

const stagedManifest = await stageCandidate(candidateRoot, releaseDigest, manifest);
await writeJsonAtomically(evidencePath, stagedManifest, 0o644);
console.log(`built immutable public-beta candidate ${releaseDigest}`);
console.log(`candidate: ${resolve(candidateRoot, releaseDigest)}`);
console.log(`manifest: ${evidencePath}`);
console.log(
  `source=${source.digest} runtime=${runtime.digest} dependencies=${dependencies.digest} artifacts=${artifactSet.digest}`,
);

async function collectSourceFiles() {
  const roots = [
    ".env.example",
    ".github",
    "Cargo.lock",
    "Cargo.toml",
    "README.md",
    "api",
    "apps",
    "biome.json",
    "config",
    "crates",
    "examples",
    "infra",
    "package-lock.json",
    "package.json",
    "packages",
    "scripts",
    "security",
    "services",
    "tsconfig.base.json",
    "tsconfig.json",
  ];
  const files = [];
  for (const sourceRoot of roots) {
    const absolute = resolve(repositoryRoot, sourceRoot);
    const stat = await lstat(absolute);
    if (stat.isFile()) files.push(sourceRoot);
    else if (stat.isDirectory()) await walkSource(absolute, files);
    else throw new Error(`source input is not a regular file or directory: ${sourceRoot}`);
  }
  return files.sort();
}

async function walkSource(directory, files) {
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    const absolute = join(directory, entry.name);
    const repositoryPath = toRepositoryPath(absolute);
    if (entry.isDirectory()) {
      if (isGeneratedDirectory(entry.name)) continue;
      await walkSource(absolute, files);
    } else if (entry.isFile()) files.push(repositoryPath);
    else throw new Error(`source tree contains unsupported entry: ${repositoryPath}`);
  }
}

function isGeneratedDirectory(name) {
  return new Set([
    ".git",
    ".local",
    ".playwright-tmp",
    "coverage",
    "dist",
    "node_modules",
    "playwright-report",
    "target",
    "test-results",
    "web-dist",
  ]).has(name);
}

async function collectArtifacts() {
  // The candidate packages target/release and the console's web-dist
  // verbatim, so build both here: a candidate quietly staged from
  // yesterday's binaries once shipped a release whose services did not
  // contain the sources it claimed to, and a stale console bundle would
  // ship a UI calling routes the services no longer serve.
  for (const [command, args] of [
    ["cargo", ["build", "--release", "--workspace", "--bins"]],
    ["npm", ["run", "build", "--workspace", "@mako-cloud/console"]],
  ]) {
    await new Promise((resolvePromise, rejectPromise) => {
      const build = spawn(command, args, {
        cwd: repositoryRoot,
        stdio: ["ignore", "inherit", "inherit"],
      });
      build.on("error", rejectPromise);
      build.on("exit", (code) =>
        code === 0
          ? resolvePromise()
          : rejectPromise(new Error(`${command} ${args.join(" ")} exited with ${code}`)),
      );
    });
  }
  const files = [];
  for (const binary of [
    "mako-control-plane",
    "mako-control-session",
    "mako-operator-session",
    "mako-operator-admin",
    "mako-operator-projection",
    "mako-data-plane",
    "mako-edge-gateway",
    "mako-qualification-fixture",
    "mako-telemetry-query",
    "mako-storage-ops",
    "mako-control-storage-ops",
  ]) {
    const sourcePath = `target/release/${binary}`;
    const stat = await requireRegularFile(sourcePath);
    if ((stat.mode & 0o111) === 0)
      throw new Error(`candidate binary is not executable: ${sourcePath}`);
    files.push(await describeFile(sourcePath, `bin/${binary}`, 0o755));
  }
  const consoleRoot = resolve(repositoryRoot, "apps/console/web-dist");
  const consoleFiles = [];
  await walkArtifact(consoleRoot, consoleFiles);
  if (!consoleFiles.includes("index.html")) {
    throw new Error(
      "console candidate is missing apps/console/web-dist/index.html. It is generated, " +
        "not committed. Run: npm run build --workspace @mako-cloud/console",
    );
  }
  for (const consoleFile of consoleFiles.sort()) {
    const sourcePath = `apps/console/web-dist/${consoleFile}`;
    files.push(await describeFile(sourcePath, `console/${consoleFile}`, 0o644));
  }
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

async function walkArtifact(directory, files, prefix = "") {
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    const artifactPath = prefix === "" ? entry.name : `${prefix}/${entry.name}`;
    if (entry.isDirectory()) await walkArtifact(join(directory, entry.name), files, artifactPath);
    else if (entry.isFile()) files.push(artifactPath);
    else throw new Error(`console artifact contains unsupported entry: ${artifactPath}`);
  }
}

async function describeNamedFiles(paths) {
  const files = await describeFiles(paths);
  return { digest: files.digest, files: files.files };
}

async function describeSourceFiles(paths) {
  const files = [];
  for (const path of paths) {
    if (path !== deploymentSelectorPath) {
      files.push(await describeRepositoryFile(path));
      continue;
    }

    const stat = await requireRegularFile(path);
    const contents = await readFile(resolve(repositoryRoot, path), "utf8");
    const selectorPattern = /^mako_release_digest:[^\r\n]*$/gm;
    const selectors = [...contents.matchAll(selectorPattern)];
    if (selectors.length !== 1) {
      throw new Error(
        `${deploymentSelectorPath} must contain exactly one mako_release_digest selector`,
      );
    }
    const normalized = contents.replace(
      selectorPattern,
      "mako_release_digest: <deployment-selector>",
    );
    files.push({
      path,
      sha256: sha256(normalized),
      size: Buffer.byteLength(normalized),
      mode: stat.mode & 0o777,
    });
  }
  return {
    digest: digestEntries(files),
    files,
    normalizations: [
      {
        path: deploymentSelectorPath,
        key: "mako_release_digest",
        reason: "deployment selector is derived from this release identity",
      },
    ],
  };
}

async function describeFiles(paths) {
  const files = [];
  for (const path of paths) {
    files.push(await describeRepositoryFile(path));
  }
  return { digest: digestEntries(files), files };
}

async function describeRepositoryFile(path) {
  const stat = await requireRegularFile(path);
  return {
    path,
    sha256: await hashFile(resolve(repositoryRoot, path)),
    size: stat.size,
    mode: stat.mode & 0o777,
  };
}

async function describeFile(sourcePath, installPath, mode) {
  const stat = await requireRegularFile(sourcePath);
  return {
    path: installPath,
    source: sourcePath,
    sha256: await hashFile(resolve(repositoryRoot, sourcePath)),
    size: stat.size,
    mode,
  };
}

async function collectRuntimeValues() {
  const [rustc, cargo, npm, ldd, kernel] = await Promise.all([
    commandOutput("rustc", ["--version", "--verbose"]),
    commandOutput("cargo", ["--version", "--verbose"]),
    commandOutput("npm", ["--version"]),
    commandOutput("ldd", ["--version"]),
    commandOutput("uname", ["-r"]),
  ]);
  return {
    platform: process.platform,
    architecture: process.arch,
    node: process.version,
    npm,
    rustc,
    cargo,
    libc: ldd.split("\n", 1)[0],
    buildKernel: kernel,
  };
}

async function commandOutput(command, args) {
  const { stdout } = await execFileAsync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
  });
  return stdout.trim();
}

async function stageCandidate(root, digest, releaseManifest) {
  await mkdir(root, { recursive: true, mode: 0o700 });
  const destination = resolve(root, digest);
  try {
    await lstat(destination);
    return validateStagedCandidate(destination, releaseManifest);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const temporary = resolve(root, `.staging-${digest}-${process.pid}`);
  await rm(temporary, { recursive: true, force: true });
  try {
    for (const artifact of releaseManifest.artifacts.files) {
      const source = resolve(repositoryRoot, artifact.source);
      const target = resolveInside(temporary, artifact.path);
      await mkdir(dirname(target), { recursive: true, mode: 0o755 });
      await copyFile(source, target);
      await chmod(target, artifact.mode);
    }
    await writeJsonAtomically(resolve(temporary, "manifest.json"), releaseManifest, 0o644);
    await validateStagedCandidate(temporary, releaseManifest);
    await rename(temporary, destination);
    return releaseManifest;
  } catch (error) {
    await rm(temporary, { recursive: true, force: true });
    throw error;
  }
}

async function validateStagedCandidate(directory, releaseManifest) {
  const recorded = JSON.parse(await readFile(resolve(directory, "manifest.json"), "utf8"));
  if (
    recorded.releaseDigest !== releaseManifest.releaseDigest ||
    canonicalJson(releaseIdentity(recorded)) !== canonicalJson(releaseIdentity(releaseManifest))
  ) {
    throw new Error(`existing candidate has a mismatched manifest: ${directory}`);
  }
  for (const artifact of releaseManifest.artifacts.files) {
    const target = resolveInside(directory, artifact.path);
    const stat = await lstat(target);
    if (
      !stat.isFile() ||
      stat.size !== artifact.size ||
      (await hashFile(target)) !== artifact.sha256
    ) {
      throw new Error(`staged candidate artifact failed verification: ${artifact.path}`);
    }
  }
  return recorded;
}

function releaseIdentity(manifestValue) {
  return {
    schemaVersion: manifestValue.schemaVersion,
    planHash: manifestValue.planHash,
    source: manifestValue.source,
    runtime: manifestValue.runtime,
    dependencies: manifestValue.dependencies,
    artifacts: manifestValue.artifacts,
  };
}

function resolveInside(root, path) {
  const result = resolve(root, path);
  if (result !== root && !result.startsWith(`${root}${sep}`)) {
    throw new Error(`candidate path escapes its root: ${path}`);
  }
  return result;
}

async function requireRegularFile(repositoryPath) {
  const stat = await lstat(resolve(repositoryRoot, repositoryPath));
  if (!stat.isFile()) throw new Error(`required file is not regular: ${repositoryPath}`);
  return stat;
}

async function hashFile(path) {
  return sha256(await readFile(path));
}

function digestEntries(entries) {
  return sha256(canonicalJson(entries));
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

async function writeJsonAtomically(path, value, mode) {
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.tmp-${process.pid}`;
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode });
  await rename(temporary, path);
}

function toRepositoryPath(path) {
  return relative(repositoryRoot, path).split(sep).join("/");
}

function parseOptions(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 2) {
    const argument = args[index];
    const value = args[index + 1];
    if (!argument?.startsWith("--") || value === undefined) {
      throw new Error("options require --name value pairs");
    }
    result[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
  }
  return result;
}
