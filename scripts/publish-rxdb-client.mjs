#!/usr/bin/env node
// Publish the built @mako-cloud/rxdb package to its distribution repository.
//
// The client is developed here, in the monorepo, where the conformance check
// keeps it honest against the OpenAPI contract. Applications, though, live in
// their own repositories and install it like any dependency. Until the package
// is on npm, that means a git URL, and a git URL needs a repository that holds
// an installable package at its root -- npm cannot install a subdirectory of a
// repository, and this one is private besides.
//
// So this script exports the built package -- dist, README, licence, and a
// manifest with the build-time scripts and devDependencies stripped -- into a
// public repository, commits it, and tags it with the version. Installing
// `github:makodb/mako-rxdb#v0.2.0` then behaves exactly as installing from npm
// will, which is what makes the swap later a one-line change.
//
// Usage:
//   node scripts/publish-rxdb-client.mjs [--repo <url>] [--dir <path>] [--dry-run] [--no-push]

import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageRoot = join(repositoryRoot, "packages/rxdb-client");

const options = parseArguments(process.argv.slice(2));
const distributionRepository = options.repo ?? "git@github.com:makodb/mako-rxdb.git";

const manifest = JSON.parse(readFileSync(join(packageRoot, "package.json"), "utf8"));
const version = manifest.version;
if (typeof version !== "string" || version.length === 0) {
  throw new Error("the client package has no version");
}
const tag = `v${version}`;

run("npm", ["run", "build", "-w", manifest.name], repositoryRoot);
for (const required of ["dist/browser/index.js", "dist/node/index.js", "dist/browser/index.d.ts"]) {
  if (!existsSync(join(packageRoot, required))) {
    throw new Error(`the build did not produce ${required}`);
  }
}

// A published artifact must not reach back into the monorepo. The package is
// built to be self-contained; this is where that claim is checked, because a
// git install has no registry to fail against.
// Only what the runtime and the type checker actually follow counts: an
// import in an emitted module. Source maps carry the sources verbatim, and
// those mention the development-only packages in comments; the connect
// template quotes the package's own name for the snippet it generates.
const emitted = run(
  "grep",
  ["-rlE", "(from|import|require)\\([\"']@mako-cloud/|from [\"']@mako-cloud/", join(packageRoot, "dist")],
  repositoryRoot,
  { allowFailure: true },
).trim();
const leaking = emitted
  .split("\n")
  .filter((path) => path.length > 0)
  .filter((path) => path.endsWith(".js") || path.endsWith(".d.ts"))
  .filter((path) => {
    const contents = readFileSync(path, "utf8");
    return /(?:from|import\(|require\()\s*["']@mako-cloud\/(?!rxdb["'])/.test(contents);
  });
if (leaking.length > 0) {
  throw new Error(`built files still depend on the monorepo:\n${leaking.join("\n")}`);
}

const staging = mkdtempSync(join(tmpdir(), "mako-rxdb-"));
const checkout = options.dir ?? join(staging, "repository");
try {
  if (options.dir === undefined) {
    run("git", ["clone", "--quiet", distributionRepository, checkout], staging);
  }
  // The commit and the tag both need an identity, and this checkout is
  // temporary, so give it one rather than depending on the machine's.
  run("git", ["config", "user.name", "Mako Cloud"], checkout);
  run("git", ["config", "user.email", "noreply@makodb.com"], checkout);
  // Everything the repository holds is generated, so replace rather than merge:
  // a file deleted here must disappear there.
  for (const entry of listTracked(checkout)) {
    rmSync(join(checkout, entry), { force: true, recursive: true });
  }
  mkdirSync(join(checkout, "dist"), { recursive: true });
  cpSync(join(packageRoot, "dist"), join(checkout, "dist"), { recursive: true });
  for (const file of ["README.md", "LICENSE"]) {
    cpSync(join(packageRoot, file), join(checkout, file));
  }
  writeFileSync(join(checkout, "package.json"), `${JSON.stringify(distributed(manifest), null, 2)}\n`);
  writeFileSync(
    join(checkout, ".gitignore"),
    "node_modules/\n",
  );

  if (options.dryRun) {
    process.stdout.write(`prepared ${tag} in ${checkout} (dry run, nothing committed)\n`);
  } else {

    run("git", ["add", "--all"], checkout);
    const pending = run("git", ["status", "--porcelain"], checkout).trim();
    if (pending.length === 0) {
      process.stdout.write(`${tag} is already published and unchanged\n`);
    } else {
      run("git", ["commit", "--quiet", "--message", commitMessage(version)], checkout);
    }
    const tagged = run("git", ["tag", "--list", tag], checkout).trim();
    if (tagged.length === 0) {
      run("git", ["tag", "--annotate", tag, "--message", `@mako-cloud/rxdb ${version}`], checkout);
    }
    if (!options.noPush) {
      run("git", ["push", "--quiet", "origin", "HEAD:main"], checkout);
      run("git", ["push", "--quiet", "origin", tag], checkout);
      process.stdout.write(`published ${tag} to ${distributionRepository}\n`);
    } else {
      process.stdout.write(`prepared ${tag} in ${checkout} (not pushed)\n`);
    }
  }

} finally {
  if (options.dir === undefined && !options.dryRun) {
    rmSync(staging, { force: true, recursive: true });
  }
}

/** The manifest an application installs: no build, no monorepo. */
function distributed(source) {
  const {
    scripts: _scripts,
    devDependencies: _devDependencies,
    private: _private,
    ...rest
  } = source;
  return {
    ...rest,
    repository: { type: "git", url: "git+https://github.com/makodb/mako-rxdb.git" },
    homepage: "https://github.com/makodb/mako-rxdb#readme",
    bugs: { url: "https://github.com/makodb/mako-rxdb/issues" },
  };
}

function commitMessage(released) {
  return `@mako-cloud/rxdb ${released}

Built from the platform repository. This repository holds no sources: it is
the installable form of the client, so an application can depend on it by git
URL until the package is published to npm.`;
}

function listTracked(checkout) {
  const tracked = run("git", ["ls-files"], checkout, { allowFailure: true }).trim();
  const entries = tracked.split("\n").filter((entry) => entry.length > 0);
  return [...new Set(entries.map((entry) => entry.split("/")[0]))];
}

function run(command, args, cwd, { allowFailure = false } = {}) {
  try {
    return execFileSync(command, args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch (error) {
    if (allowFailure) {
      return "";
    }
    process.stderr.write(String(error.stderr ?? ""));
    throw new Error(`${command} ${args.join(" ")} failed`);
  }
}

function parseArguments(argv) {
  const parsed = { dryRun: false, noPush: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--dry-run") {
      parsed.dryRun = true;
    } else if (argument === "--no-push") {
      parsed.noPush = true;
    } else if (argument === "--repo") {
      index += 1;
      parsed.repo = argv[index];
    } else if (argument === "--dir") {
      index += 1;
      parsed.dir = resolve(argv[index] ?? "");
    } else {
      throw new Error(`unknown argument ${argument}`);
    }
  }
  return parsed;
}
