#!/usr/bin/env node
// Export the Rational example as the standalone application repository.
//
// Rational is developed here, in the platform repository, where it is the
// standing proof that an application can be built on nothing but what Mako
// Cloud offers: every screen runs against the example's own browser tests, and
// every gap it hits is fixed in the platform rather than worked around in the
// app. But an example that only builds inside this workspace proves nothing to
// anybody outside it. So the app also lives in a public repository of its own,
// where `git clone && npm install && npm run build` works with no access to
// this one -- the client comes from `github:makodb/mako-rxdb`, the database is
// a Mako Cloud project, and the only server-side code is an edge function.
//
// That public repository is generated, never hand-maintained: this script
// copies the sources verbatim, writes the two files that differ between a
// workspace member and a standalone application (`package.json`, which names
// real versions instead of `*`, and `vite.config.ts`, which serves from a
// GitHub Pages subpath and reads `rational.config.json`), and refuses to
// publish a tree that carries a credential or a path into this repository.
//
// Usage:
//   node scripts/export-rational-app.mjs [--repo <url>] [--dir <path>] [--dry-run] [--no-push]

import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const exampleRoot = join(repositoryRoot, "examples/rational");

/** The application repository, as it is named on GitHub and served by Pages. */
const APPLICATION = {
  name: "rational",
  owner: "shuaimu",
  description: "A household money manager with no backend server of its own",
  license: "Apache-2.0",
};
const homepage = `https://${APPLICATION.owner}.github.io/${APPLICATION.name}/`;
/** A GitHub project page is served from a subpath, and every asset is under it. */
const pagesBase = `/${APPLICATION.name}/`;
/** Where `vite build` writes the site. `tsc -b` owns `dist`, and the unit tests read it. */
const siteDirectory = "web-dist";
/** The published, installable form of the client. See scripts/publish-rxdb-client.mjs. */
const clientSpecifier = "github:makodb/mako-rxdb#v0.1.0";

// Copied verbatim. Directories are copied whole, minus the names below.
const COPIED = [
  "src",
  "functions",
  "mako",
  "test",
  "test-unit",
  "test-support",
  "index.html",
  "playwright.config.ts",
];
/** Copied from `scripts/`: the two entry points, their data, and their types. */
const COPIED_SCRIPTS = (entries) =>
  entries.filter(
    (entry) =>
      ["bootstrap.mjs", "seed.mjs", "demo-data.mjs"].includes(entry) || entry.endsWith(".d.mts"),
  );
/** Rewritten rather than copied: they name this workspace. */
const REWRITTEN_TSCONFIGS = ["tsconfig.json", "tsconfig.test.json"];
/** Generated: what a workspace member does not need to say and an application does. */
const GENERATED = ["package.json", "vite.config.ts"];
/**
 * Never exported, at any depth. Build output and caches are noise; the live
 * suite and the environment file drive this repository's own binaries; the
 * findings log is an internal engineering record about the platform.
 */
const NEVER = new Set([
  "node_modules",
  "dist",
  siteDirectory,
  "test-results",
  "playwright-report",
  ".playwright-tmp",
  ".vite",
  "mako.env.json",
  ".live-tenant.json",
  "test-live",
  "playwright.live.config.ts",
  "PLATFORM-FINDINGS.md",
  "rational.config.json",
]);
/**
 * The application repository's own files: its prose, its licence, the workflow
 * that publishes it, the lock file its own `npm install` resolved -- which the
 * export must not delete, because the published build runs `npm ci` -- and the
 * configuration naming the project the published site talks to. They are
 * reviewed and edited there, so the export neither writes nor deletes them;
 * everything else it replaces.
 */
const PRESERVED = new Set([
  ".git",
  ".github",
  ".gitignore",
  "LICENSE",
  "README.md",
  "package-lock.json",
  "rational.config.json",
  "rational.config.example.json",
]);

/**
 * The environment file is named for the platform in this workspace and for the
 * application in its own repository. One substitution, reported file by file.
 */
const ENVIRONMENT_FILE = { from: "mako.env.json", to: "rational.config.json" };

/**
 * Configuration that names no project. Kept identical to `PLACEHOLDER` in
 * `src/config.ts`: the app reads these ids as "no project" and runs against
 * its in-browser fake backend, which is what makes the published site a demo
 * instead of a broken client.
 */
const PLACEHOLDER = /(?:^|[._-])replace[_-]?me$/iu;

/** What must never reach a public repository. */
const SECRETS = [
  { what: "a service credential", pattern: /mako_sk\.[A-Za-z0-9_-]{6,}/u },
  { what: "an automation token", pattern: /mako_at\.[A-Za-z0-9_-]{6,}/u },
  { what: "a refresh token", pattern: /mako_rt\.[A-Za-z0-9_-]{6,}/u },
  { what: "a function secret", pattern: /mako_fn\.[A-Za-z0-9_-]{6,}/u },
  { what: "a private key", pattern: /-----BEGIN [A-Z ]*PRIVATE KEY-----/u },
  { what: "a bearer token", pattern: /Bearer\s+ey[A-Za-z0-9_.-]{10,}/u },
  { what: "an absolute path on the machine that ran the export", pattern: /\/home\/[a-z]/u },
];
const TEXT = /\.(?:ts|tsx|mts|mjs|js|jsx|json|html|css|md|yml|yaml)$/u;

const options = parseArguments(process.argv.slice(2));
const distributionRepository =
  options.repo ?? `git@github.com:${APPLICATION.owner}/${APPLICATION.name}.git`;
// Default to a sibling of this repository, which is where the application
// checkout lives; `--dir` overrides it and a missing directory is cloned.
const checkout = options.dir ?? resolve(repositoryRoot, "..", APPLICATION.name);

const manifest = JSON.parse(readFileSync(join(exampleRoot, "package.json"), "utf8"));
const workspaceManifest = JSON.parse(readFileSync(join(repositoryRoot, "package.json"), "utf8"));

if (!existsSync(checkout)) {
  mkdirSync(dirname(checkout), { recursive: true });
  run("git", ["clone", "--quiet", distributionRepository, checkout], dirname(checkout));
}
if (!existsSync(join(checkout, ".git"))) {
  throw new Error(`${checkout} is not a git checkout`);
}
// The commit needs an identity and this checkout may be a throwaway, so give
// it one rather than depending on the machine's.
run("git", ["config", "user.name", "Mako Cloud"], checkout);
run("git", ["config", "user.email", "noreply@makodb.com"], checkout);

// Everything the export owns is generated, so replace rather than merge: a
// file deleted here must disappear there. The repository's own files stay.
for (const entry of listTracked(checkout)) {
  if (PRESERVED.has(entry)) continue;
  rmSync(join(checkout, entry), { force: true, recursive: true });
}

/** @type {string[]} */
const exported = [];
for (const entry of COPIED) {
  exported.push(...copy(join(exampleRoot, entry), join(checkout, entry)));
}
mkdirSync(join(checkout, "scripts"), { recursive: true });
for (const entry of COPIED_SCRIPTS(readdirSync(join(exampleRoot, "scripts")).sort())) {
  exported.push(...copy(join(exampleRoot, "scripts", entry), join(checkout, "scripts", entry)));
}

// `src` and `scripts` name the environment file; nothing else does.
const renamed = [];
for (const path of exported) {
  if (!path.startsWith("src/") && !path.startsWith("scripts/")) continue;
  if (!TEXT.test(path)) continue;
  const absolute = join(checkout, path);
  const before = readFileSync(absolute, "utf8");
  const occurrences = before.split(ENVIRONMENT_FILE.from).length - 1;
  if (occurrences === 0) continue;
  writeFileSync(absolute, before.split(ENVIRONMENT_FILE.from).join(ENVIRONMENT_FILE.to));
  renamed.push({ path, occurrences });
}

for (const name of REWRITTEN_TSCONFIGS) {
  writeFileSync(join(checkout, name), `${JSON.stringify(standaloneTsconfig(name), null, 2)}\n`);
  exported.push(name);
}
writeFileSync(join(checkout, "package.json"), `${JSON.stringify(applicationManifest(), null, 2)}\n`);
writeFileSync(join(checkout, "vite.config.ts"), viteConfig());
exported.push(...GENERATED);
exported.sort();

verify(checkout, exported);

process.stdout.write(`${exported.length} files exported to ${checkout}\n`);
for (const path of exported) process.stdout.write(`  ${path}\n`);
process.stdout.write(`renamed ${ENVIRONMENT_FILE.from} -> ${ENVIRONMENT_FILE.to} in:\n`);
for (const entry of renamed) {
  process.stdout.write(`  ${entry.path} (${entry.occurrences})\n`);
}

if (options.dryRun) {
  process.stdout.write(`prepared ${checkout} (dry run, nothing committed)\n`);
} else {
  run("git", ["add", "--all"], checkout);
  const pending = run("git", ["status", "--porcelain"], checkout).trim();
  if (pending.length === 0) {
    process.stdout.write("the application repository is already up to date\n");
  } else {
    run("git", ["commit", "--quiet", "--message", commitMessage()], checkout);
  }
  if (options.noPush) {
    process.stdout.write(`prepared ${checkout} (not pushed)\n`);
  } else {
    run("git", ["push", "--quiet", "origin", "HEAD:main"], checkout);
    process.stdout.write(`pushed ${APPLICATION.name} to ${distributionRepository}\n`);
  }
}

/** Copy one file or directory, minus the names that are never exported. */
function copy(from, to) {
  if (NEVER.has(basename(from))) return [];
  if (statSync(from).isDirectory()) {
    const written = [];
    mkdirSync(to, { recursive: true });
    for (const entry of readdirSync(from).sort()) {
      written.push(...copy(join(from, entry), join(to, entry)));
    }
    return written;
  }
  mkdirSync(dirname(to), { recursive: true });
  cpSync(from, to);
  return [relative(checkout, to)];
}

/**
 * The manifest an application publishes: the same scripts minus the one that
 * drives this repository's binaries, the client by its published git URL, and
 * every version spelled out -- including TypeScript, which a workspace member
 * inherits from the root and a standalone application must ask for.
 */
function applicationManifest() {
  const scripts = {};
  for (const [name, command] of Object.entries(manifest.scripts)) {
    if (name === "test:browser-live") continue;
    scripts[name] = command;
  }
  const dependencies = {};
  for (const [name, range] of Object.entries(manifest.dependencies)) {
    dependencies[name] = name === "@mako-cloud/rxdb" ? clientSpecifier : range;
  }
  return {
    name: APPLICATION.name,
    version: manifest.version,
    description: APPLICATION.description,
    license: APPLICATION.license,
    repository: {
      type: "git",
      url: `git+https://github.com/${APPLICATION.owner}/${APPLICATION.name}.git`,
    },
    homepage,
    bugs: { url: `https://github.com/${APPLICATION.owner}/${APPLICATION.name}/issues` },
    type: "module",
    scripts,
    dependencies,
    devDependencies: sorted({
      ...manifest.devDependencies,
      typescript: workspaceManifest.devDependencies.typescript,
    }),
    engines: { node: ">=20.19.0" },
  };
}

/** A dependency map in the order npm writes one, so a lock file compares equal. */
function sorted(entries) {
  return Object.fromEntries(Object.entries(entries).sort(([left], [right]) => (left < right ? -1 : 1)));
}

/**
 * The example's TypeScript configuration with the workspace taken out of it:
 * the shared base inlined, the project reference to the client package
 * dropped (there it is built from source, here it is an installed package),
 * and any file that is not exported dropped from `include`.
 */
function standaloneTsconfig(name) {
  const source = JSON.parse(readFileSync(join(exampleRoot, name), "utf8"));
  const base = JSON.parse(
    readFileSync(resolve(dirname(join(exampleRoot, name)), source.extends), "utf8"),
  );
  const { extends: _extends, references: _references, include, ...rest } = source;
  return {
    ...rest,
    compilerOptions: { ...base.compilerOptions, ...source.compilerOptions },
    ...(include === undefined
      ? {}
      : { include: include.filter((entry) => !NEVER.has(entry.split("/")[0])) }),
  };
}

/** The build a standalone application needs, which a workspace member does not. */
function viteConfig() {
  return `import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { defineConfig, type ProxyOptions } from "vite";

/**
 * Rational is a static site, so the project it talks to is compiled into the
 * bundle. \`rational.config.json\` names that project -- endpoint, ids, and the
 * public project key, all of them public values, and this repository commits
 * the one the published site uses. A checkout without it falls back to the
 * example file, whose ids are placeholders; \`src/config.ts\` reads those as "no
 * project" and runs the app entirely against its in-browser fake backend, so a
 * fork builds and runs before it has a project of its own.
 *
 * \`base\` is \`${pagesBase}\` because GitHub Pages serves a project page from a
 * subpath, and every asset the built \`index.html\` names has to be under it.
 *
 * The data plane sends no CORS headers, so in development the dev server
 * proxies \`/v1\` (and the function route) to the configured endpoint and the
 * app calls same-origin -- the topology a deployment has behind its reverse
 * proxy. A deployed site calls the endpoint cross-origin instead, which is
 * what \`mako allowed-origins set --origin\` allows.
 */
interface RationalConfigFile {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly publicProjectKey: string;
  /** Where the edge gateway serves the environment's functions, when deployed. */
  readonly functionsEndpoint?: string | null;
  readonly signIn?: {
    readonly providers: ReadonlyArray<{
      readonly name: string;
      readonly enabled: boolean;
      readonly label?: string;
    }>;
    readonly magicLinks: boolean;
  };
}

function readConfigFile(): RationalConfigFile | null {
  for (const name of ["rational.config.json", "rational.config.example.json"]) {
    const path = fileURLToPath(new URL(\`./\${name}\`, import.meta.url));
    if (existsSync(path)) return JSON.parse(readFileSync(path, "utf8")) as RationalConfigFile;
  }
  return null;
}

export default defineConfig(({ command }) => {
  const configFile = readConfigFile();
  const liveEndpoint = process.env.MAKO_LIVE_ENDPOINT ?? configFile?.endpoint;
  const functionsEndpoint =
    process.env.MAKO_FUNCTIONS_ENDPOINT ?? configFile?.functionsEndpoint ?? undefined;
  const runtimeEnvironment =
    configFile === null
      ? null
      : {
          ...configFile,
          endpoint: command === "serve" ? "same-origin" : configFile.endpoint,
          functionsEndpoint:
            command === "serve" && functionsEndpoint !== undefined
              ? "same-origin"
              : (configFile.functionsEndpoint ?? null),
        };
  const proxy: Record<string, ProxyOptions> = {};
  if (liveEndpoint !== undefined) {
    proxy["/v1"] = {
      target: liveEndpoint,
      changeOrigin: false,
      configure: (server) => {
        server.on("proxyRes", (proxyRes) => {
          // The live pull stream is server-sent events; never buffer it.
          if (proxyRes.headers["content-type"]?.includes("text/event-stream")) {
            proxyRes.headers["cache-control"] = "no-cache";
          }
        });
      },
    };
  }
  if (functionsEndpoint !== undefined && functionsEndpoint !== null) {
    proxy["^/[^/]+--[^/]+/functions/v1/"] = {
      target: functionsEndpoint,
      changeOrigin: false,
    };
  }
  return {
    base: "${pagesBase}",
    define: { __RATIONAL_ENV__: JSON.stringify(runtimeEnvironment) },
    build: { outDir: "${siteDirectory}", emptyOutDir: true },
    server: Object.keys(proxy).length === 0 ? {} : { proxy },
  };
});
`;
}

/**
 * What a public repository must be able to claim about itself: it carries no
 * credential, no path into this workspace, and a demo that runs.
 */
function verify(root, files) {
  const credential = readFileSync(join(root, "functions/households/credential.ts"), "utf8");
  if (!credential.includes("this deployment carries no households service credential")) {
    throw new Error(
      "functions/households/credential.ts does not carry the fail-closed placeholder: the" +
        " deploy-time rewrite has leaked into the sources, and a credential would be published",
    );
  }
  for (const path of files) {
    if (!TEXT.test(path)) continue;
    const contents = readFileSync(join(root, path), "utf8");
    for (const { what, pattern } of SECRETS) {
      const found = pattern.exec(contents);
      if (found !== null) throw new Error(`${path} carries ${what}: ${found[0]}`);
    }
    // The client is a published package, so its scope is expected; the name of
    // this repository, anywhere else, is a reference nobody outside it can follow.
    if (contents.replaceAll("@mako-cloud/", "").includes("mako-cloud")) {
      throw new Error(`${path} names the platform repository`);
    }
  }

  // The example configuration is what a checkout with no project of its own
  // builds against, and it must keep pointing nowhere. If it ever named a real
  // project, every fork would come up as a client for somebody else's data.
  const example = JSON.parse(readFileSync(join(root, "rational.config.example.json"), "utf8"));
  for (const field of ["projectId", "environmentId", "publicProjectKey"]) {
    if (!PLACEHOLDER.test(example[field])) {
      throw new Error(
        `rational.config.example.json ${field} names a real project (${example[field]});` +
          " the published site would try to use it instead of the in-browser demo",
      );
    }
  }

  // The published build installs from the lock file, so it has to be there and
  // it has to agree with the manifest this export just wrote.
  const lockPath = join(root, "package-lock.json");
  const generated = applicationManifest();
  if (!existsSync(lockPath)) {
    process.stderr.write(
      "warning: there is no package-lock.json; run `npm install` in the application" +
        " repository and commit it, or the published build has nothing to install from\n",
    );
  } else {
    const lock = JSON.parse(readFileSync(lockPath, "utf8"));
    const locked = lock.packages?.[""] ?? {};
    for (const field of ["dependencies", "devDependencies"]) {
      const wanted = JSON.stringify(sorted(generated[field] ?? {}));
      if (JSON.stringify(sorted(locked[field] ?? {})) !== wanted) {
        throw new Error(
          `package-lock.json is out of date with the generated package.json (${field});` +
            " run `npm install` in the application repository and commit the result",
        );
      }
    }
  }

  // `tsc -b` writes `dist` and the unit tests read it, so the site is built
  // elsewhere -- and the workflow has to publish the directory it is built in.
  const workflow = join(root, ".github/workflows/pages.yml");
  if (existsSync(workflow)) {
    const published = /^\s*path:\s*(\S+)\s*$/mu.exec(readFileSync(workflow, "utf8"));
    if (published !== null && published[1] !== siteDirectory) {
      throw new Error(
        `.github/workflows/pages.yml publishes ${published[1]}, but the build writes ${siteDirectory}`,
      );
    }
  }
}

function commitMessage() {
  return `Regenerate Rational from the platform repository

Exported by scripts/export-rational-app.mjs. The sources here are generated:
they are developed in the platform repository, where the app's tests run
against the platform itself, and pull requests belong there.`;
}

function listTracked(root) {
  const tracked = run("git", ["ls-files"], root, { allowFailure: true }).trim();
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
