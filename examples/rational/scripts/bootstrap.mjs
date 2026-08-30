#!/usr/bin/env node
/**
 * Bootstrap a Rational tenant on a Mako Cloud environment through the `mako`
 * CLI: create or reuse a project and an environment, publish every
 * collection of `mako/collections.json` with its indexes, activate the
 * policies of `mako/policies`, create the `receipts` bucket, initialize the
 * signing key, issue a public key, read the environment's sign-in settings,
 * and write `mako.env.json` for the app. With --functions it also deploys
 * `functions/households` with a service credential scoped to what that
 * function writes. Then, unless --skip-users, it registers the demo
 * application users, gives them household claims, and seeds the household and
 * membership documents through a short-lived service credential — the same
 * writes the function makes, so an environment with no function runtime still
 * has a working demo household.
 *
 * Re-running reuses everything it finds. Usage:
 *
 *   node scripts/bootstrap.mjs --endpoint http://127.0.0.1:8081 \
 *     --data-endpoint http://127.0.0.1:8080 [--token <developer session>] \
 *     [--functions --functions-endpoint http://127.0.0.1:8082]
 *
 * Without --token / MAKO_TOKEN the CLI's stored profile session is used.
 */
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const exampleRoot = resolve(here, "..");
const repositoryRoot = resolve(exampleRoot, "../..");

/**
 * The `mako` CLI to drive. A checkout that installs it as a dependency has it
 * in a local `node_modules/.bin`; in this repository that is the workspace
 * root. Otherwise it is whatever `mako` is on the PATH — an installed CLI.
 */
function localCli() {
  for (const root of [exampleRoot, repositoryRoot]) {
    const candidate = join(root, "node_modules", ".bin", "mako");
    if (existsSync(candidate)) return candidate;
  }
  return "mako";
}

const DEFAULTS = {
  endpoint: process.env.MAKO_ENDPOINT,
  token: process.env.MAKO_TOKEN,
  "token-kind": process.env.MAKO_TOKEN_KIND ?? "developer_session",
  "data-endpoint": process.env.MAKO_DATA_ENDPOINT ?? "http://127.0.0.1:8080",
  "project-name": "Rational",
  "env-name": "development",
  region: "local",
  team: undefined,
  profile: process.env.MAKO_PROFILE,
  output: join(exampleRoot, "mako.env.json"),
  "household-id": "hh_demo",
  "household-name": "Demo household",
  currency: "USD",
  "owner-email": "owner@rational.test",
  "owner-password": "RationalDemo1!",
  "editor-email": "editor@rational.test",
  "editor-password": "RationalDemo1!",
  cli: process.env.MAKO_CLI ?? localCli(),
  "skip-users": false,
  functions: false,
  "functions-endpoint": process.env.MAKO_FUNCTIONS_ENDPOINT ?? "http://127.0.0.1:8082",
  "function-name": "households",
  "config-dir": process.env.MAKO_CONFIG_DIR,
};

function parseArguments(argv) {
  const options = { ...DEFAULTS };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith("--")) fail(`unexpected argument ${argument}`);
    const name = argument.slice(2);
    if (name === "help") {
      console.log(readFileSync(fileURLToPath(import.meta.url), "utf8").split("*/")[0]);
      process.exit(0);
    }
    if (!(name in DEFAULTS)) fail(`unknown option --${name}`);
    if (typeof DEFAULTS[name] === "boolean") {
      options[name] = true;
      continue;
    }
    const value = argv[index + 1];
    if (value === undefined) fail(`--${name} needs a value`);
    options[name] = value;
    index += 1;
  }
  if (options.endpoint === undefined) fail("--endpoint (or MAKO_ENDPOINT) is required");
  return options;
}

function fail(message) {
  console.error(`bootstrap: ${message}`);
  process.exit(1);
}

function log(message) {
  console.error(`bootstrap: ${message}`);
}

const options = parseArguments(process.argv.slice(2));
const scratch = mkdtempSync(join(tmpdir(), "rational-bootstrap-"));
process.on("exit", () => rmSync(scratch, { recursive: true, force: true }));

const cliEnvironment = {
  ...process.env,
  MAKO_ENDPOINT: options.endpoint,
  ...(options.token === undefined
    ? {}
    : { MAKO_TOKEN: options.token, MAKO_TOKEN_KIND: options["token-kind"] }),
  ...(options.profile === undefined ? {} : { MAKO_PROFILE: options.profile }),
  ...(options["config-dir"] === undefined ? {} : { MAKO_CONFIG_DIR: options["config-dir"] }),
  MAKO_WAIT_INTERVAL_MS: process.env.MAKO_WAIT_INTERVAL_MS ?? "500",
};

/** Run `mako …` and return {status, stdout, stderr}; never throws. */
function mako(args) {
  const executable = options.cli;
  const command = executable.endsWith(".js") ? "node" : executable;
  const commandArgs = executable.endsWith(".js") ? [executable, ...args] : args;
  const result = spawnSync(command, commandArgs, {
    env: cliEnvironment,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) fail(`could not run ${executable}: ${result.error.message}`);
  return { status: result.status ?? -1, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

/** Run a command with --json and parse its output; exit codes in `allow` return null. */
function makoJson(args, allow = []) {
  const run = mako([...args, "--json"]);
  if (run.status !== 0) {
    if (allow.includes(run.status)) return null;
    fail(`mako ${args.join(" ")} failed (exit ${run.status}): ${run.stderr.trim()}`);
  }
  try {
    return JSON.parse(run.stdout);
  } catch {
    fail(`mako ${args.join(" ")} did not print JSON: ${run.stdout.slice(0, 400)}`);
  }
  return null;
}

function items(value) {
  if (Array.isArray(value)) return value;
  if (value && Array.isArray(value.items)) return value.items;
  return [];
}

function tempJson(name, value) {
  const path = join(scratch, name);
  writeFileSync(path, JSON.stringify(value));
  return `@${path}`;
}

function deepEqual(left, right) {
  return JSON.stringify(sortKeys(left)) === JSON.stringify(sortKeys(right));
}

function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortKeys(value[key])]),
    );
  }
  return value;
}

// --- 1. Developer session ------------------------------------------------

const status = makoJson(["auth", "status"]);
log(`signed in via ${status?.credential ?? "unknown credential"} at ${options.endpoint}`);

// --- 2. Project and environment -----------------------------------------

function findProject() {
  const listed = items(makoJson(["projects", "list", ...(options.team ? ["--team", options.team] : [])]));
  return listed.find(
    (project) =>
      project.name === options["project-name"] && (project.state === "active" || project.state === "creating"),
  );
}

let project = findProject();
if (project === undefined) {
  log(`creating project "${options["project-name"]}" in region ${options.region}`);
  project = makoJson([
    "projects",
    "create",
    options["project-name"],
    "--region",
    options.region,
    ...(options.team ? ["--team", options.team] : []),
    "--wait",
  ]);
} else {
  log(`reusing project ${project.id}`);
}
if (project.state !== "active") fail(`project ${project.id} is ${project.state}, not active`);
const projectId = project.id;

let environment = items(makoJson(["envs", "list", "--project", projectId])).find(
  (candidate) => candidate.name === options["env-name"] && candidate.state !== "deleting",
);
if (environment === undefined) {
  log(`creating environment "${options["env-name"]}"`);
  environment = makoJson(["envs", "create", options["env-name"], "--project", projectId, "--wait"]);
} else {
  log(`reusing environment ${environment.id}`);
}
if (environment.state !== "active") fail(`environment ${environment.id} is ${environment.state}`);
const environmentId = environment.id;
const tenant = ["--project", projectId, "--env", environmentId];

// --- 3. Collections, indexes, policies ----------------------------------

const model = JSON.parse(readFileSync(join(exampleRoot, "mako", "collections.json"), "utf8"));
const existingCollections = new Map(
  items(makoJson(["collections", "list", ...tenant])).map((collection) => [collection.id, collection]),
);

for (const collection of model.collections) {
  const existing = existingCollections.get(collection.id);
  if (existing === undefined) {
    log(`creating collection ${collection.id}`);
    makoJson([
      "collections",
      "create",
      collection.id,
      "--schema",
      tempJson(`${collection.id}.schema.json`, collection.jsonSchema),
      "--primary-key",
      collection.primaryKey.field,
      "--schema-version",
      String(model.schemaVersion),
      ...tenant,
    ]);
  } else if (!deepEqual(existing.jsonSchema, collection.jsonSchema)) {
    log(
      `collection ${collection.id} exists with a different schema (version ${existing.schemaVersion}); ` +
        "publish a new schema version by hand (mako collections schema publish) — leaving it as is",
    );
  } else {
    log(`collection ${collection.id} is up to date`);
  }

  const indexes = items(makoJson(["indexes", "list", collection.id, ...tenant]));
  for (const index of collection.indexes) {
    const present = indexes.find(
      (candidate) =>
        candidate.name === index.name &&
        candidate.state !== "failed" &&
        candidate.state !== "deleting",
    );
    if (present !== undefined) continue;
    log(`creating index ${collection.id}/${index.name} on (${index.fields.join(", ")})`);
    makoJson([
      "indexes",
      "create",
      collection.id,
      "--name",
      index.name,
      "--version",
      "1",
      ...index.fields.flatMap((field) => ["--field", field]),
      ...tenant,
    ]);
  }

  const desired = JSON.parse(
    readFileSync(join(exampleRoot, "mako", "policies", `${collection.id}.json`), "utf8"),
  );
  const active = makoJson(["policies", "get", collection.id, ...tenant], [4]);
  const activeRules = active?.policy?.state === "active" ? active.policy.rules : null;
  if (activeRules !== null && deepEqual(activeRules, desired.rules)) {
    log(`policy of ${collection.id} is active and up to date (version ${active.policy.version})`);
    continue;
  }
  let version = (active?.policy?.version ?? 0) + 1;
  let drafted = null;
  for (let attempt = 0; attempt < 10 && drafted === null; attempt += 1) {
    const draft = mako([
      "policies",
      "draft",
      collection.id,
      "--input",
      tempJson(`${collection.id}.policy.${version}.json`, { version, rules: desired.rules }),
      ...tenant,
      "--json",
    ]);
    if (draft.status === 0) {
      drafted = version;
    } else if (draft.status === 5) {
      version += 1;
    } else {
      fail(`policy draft for ${collection.id} failed (exit ${draft.status}): ${draft.stderr.trim()}`);
    }
  }
  if (drafted === null) fail(`could not find a free policy version for ${collection.id}`);
  const validation = makoJson(["policies", "validate", collection.id, String(drafted), ...tenant]);
  if (validation?.valid !== true) {
    const diagnostics = validation?.policy?.diagnostics ?? [];
    fail(
      `policy version ${drafted} of ${collection.id} did not validate:\n${diagnostics
        .map((diagnostic) => `  ${diagnostic.severity} ${diagnostic.code}: ${diagnostic.message}`)
        .join("\n")}`,
    );
  }
  log(`activating policy version ${drafted} of ${collection.id}`);
  makoJson(["policies", "activate", collection.id, String(drafted), "--yes", ...tenant]);
}

// --- 4. Bucket ----------------------------------------------------------

const bucket = JSON.parse(readFileSync(join(exampleRoot, "mako", "buckets", "receipts.json"), "utf8"));
const bucketArgs = [
  "--access",
  bucket.access,
  "--max-object-bytes",
  String(bucket.maxObjectBytes),
  ...bucket.allowedContentTypes.flatMap((type) => ["--content-type", type]),
  "--rules",
  tempJson("receipts.rules.json", bucket.rules),
  ...tenant,
];
if (makoJson(["storage", "buckets", "get", bucket.id, ...tenant], [4]) === null) {
  log(`creating bucket ${bucket.id}`);
  makoJson(["storage", "buckets", "create", bucket.id, ...bucketArgs]);
} else {
  log(`updating bucket ${bucket.id}`);
  makoJson(["storage", "buckets", "update", bucket.id, ...bucketArgs]);
}

// --- 5. Keys ------------------------------------------------------------

if (items(makoJson(["keys", "signing", "list", ...tenant])).length === 0) {
  log("initializing the JWT signing key");
  makoJson(["keys", "signing", "init", ...tenant]);
}

const previous = existsSync(options.output)
  ? JSON.parse(readFileSync(options.output, "utf8"))
  : null;
let publicProjectKey =
  previous !== null &&
  previous.projectId === projectId &&
  previous.environmentId === environmentId &&
  typeof previous.publicProjectKey === "string"
    ? previous.publicProjectKey
    : null;
if (publicProjectKey === null) {
  const keyId = `key_rational_${Date.now().toString(36)}`;
  log(`issuing public key ${keyId}`);
  const issued = makoJson(["keys", "public", "create", "--id", keyId, ...tenant]);
  publicProjectKey = issued.secret;
} else {
  log("reusing the public key from the previous mako.env.json");
}

// --- 6. Sign-in settings and the households function ---------------------

/**
 * What the app should offer on its sign-in screen. The environment is the
 * authority; an environment that answers nothing (an older control plane, or
 * a credential without the scope) leaves the app with password only.
 */
function readSignInSettings() {
  const settings = makoJson(["auth-settings", "get", ...tenant], [3, 4, 5, 7]);
  if (settings === null) {
    log("sign-in settings are unavailable; the app offers password sign-in only");
    return { providers: [], magicLinks: false };
  }
  return {
    providers: (settings.providers ?? []).map((provider) => ({
      name: provider.name,
      enabled: provider.enabled === true,
    })),
    magicLinks: settings.magicLinks?.enabled === true,
  };
}

/**
 * Deploy `functions/households` with a service credential scoped to exactly
 * what it writes. The credential is installed as the deployment's
 * `HOUSEHOLDS_SERVICE_KEY` secret with its value supplied, so nothing secret
 * is written into the uploaded bundle: the function reads it from its
 * environment, and a bundle sitting in the object store carries no
 * credential. (Until the platform accepted a supplied secret value, the
 * bootstrap had to rewrite `credential.ts` in the uploaded copy; findings
 * log #11.)
 */
function deployHouseholdsFunction() {
  const functionName = options["function-name"];
  const credentialId = `sk_rational_hh_${Date.now().toString(36)}`;
  log(`issuing the ${functionName} service credential ${credentialId}`);
  const issued = makoJson([
    "keys",
    "service",
    "create",
    "--id",
    credentialId,
    "--collection",
    "memberships",
    "--collection",
    "households",
    "--collection",
    "users",
    "--operation",
    "read",
    "--operation",
    "create",
    "--operation",
    "update",
    ...tenant,
  ]);
  const secretName = "HOUSEHOLDS_SERVICE_KEY";
  // The credential is new on every run, so the secret has to hold the new
  // value: create it the first time, rotate it to the new value after that. A
  // name is written once and cannot be freed, so rotation is the only way to
  // correct what a name holds.
  const existing = makoJson(["functions", "secrets", "get", secretName, ...tenant], [4]);
  if (existing === null) {
    log(`installing ${secretName} with the credential's value`);
    makoJson(["functions", "secrets", "create", secretName, "--value", issued.secret, ...tenant]);
  } else {
    log(`rotating ${secretName} to the new credential`);
    makoJson([
      "functions",
      "secrets",
      "rotate",
      secretName,
      "--value",
      issued.secret,
      "--yes",
      ...tenant,
    ]);
  }
  // The uploaded copy is the directory as it is committed: no credential.
  const source = join(scratch, "households-function");
  cpSync(join(exampleRoot, "functions", "households"), source, { recursive: true });
  const deployed = mako([
    "functions",
    "deploy",
    source,
    "--name",
    functionName,
    "--create",
    "--region",
    options.region,
    "--secret",
    secretName,
    "--yes",
    ...tenant,
    "--json",
  ]);
  if (deployed.status !== 0) {
    log(
      `the ${functionName} function was not deployed (exit ${deployed.status}): ` +
        `${deployed.stderr.trim().split("\n").slice(-3).join(" ")}`,
    );
    log("continuing without it: household membership stays as the bootstrap seeded it");
    return null;
  }
  log(`deployed the ${functionName} function; functions endpoint ${options["functions-endpoint"]}`);
  return options["functions-endpoint"];
}

const signIn = readSignInSettings();
const functionsEndpoint = options.functions ? deployHouseholdsFunction() : null;

const envFile = {
  endpoint: options["data-endpoint"],
  projectId,
  environmentId,
  publicProjectKey,
  functionsEndpoint,
  signIn,
};
writeFileSync(options.output, `${JSON.stringify(envFile, null, 2)}\n`);
log(`wrote ${options.output}`);

// --- 7. Demo users, claims, household, memberships -----------------------

if (options["skip-users"]) {
  console.log(JSON.stringify(envFile, null, 2));
  process.exit(0);
}

const dataBase = `${options["data-endpoint"].replace(/\/$/u, "")}/v1/projects/${projectId}/environments/${environmentId}`;

/** A data-plane request, retried while the API says to retry after a delay. */
async function dataRequest(path, init) {
  for (let attempt = 0; ; attempt += 1) {
    const response = await fetch(`${dataBase}${path}`, {
      ...init,
      headers: {
        "content-type": "application/json",
        accept: "application/json",
        "x-mako-key": publicProjectKey,
        ...(init.headers ?? {}),
      },
    });
    const text = await response.text();
    let body = null;
    try {
      body = text === "" ? null : JSON.parse(text);
    } catch {
      body = text;
    }
    const retry = body?.error?.retry;
    if (response.status >= 500 && retry?.kind === "after_delay" && attempt < 5) {
      log(`${path} answered ${response.status} (${body.error.code}); retrying in ${retry.afterMs}ms`);
      await new Promise((resolve) => setTimeout(resolve, retry.afterMs));
      continue;
    }
    return { status: response.status, body };
  }
}

async function ensureUser(email, password, role) {
  const signup = await dataRequest("/auth/signup", {
    method: "POST",
    body: JSON.stringify({ email, password }),
  });
  if (signup.status >= 400 && signup.status !== 409) {
    fail(`sign-up of ${email} failed with ${signup.status}: ${JSON.stringify(signup.body)}`);
  }
  const found = items((makoJson(["users", "search", "--query", email, ...tenant]) ?? {}).users);
  const user = found.find((candidate) => candidate.email === email) ?? found[0];
  if (user === undefined) fail(`application user ${email} was not found after sign-up`);
  const view = makoJson(["users", "get", user.id, ...tenant]);
  const trusted = { ...(view.trustedMetadata ?? {}) };
  const households = { ...(trusted.households ?? {}) };
  if (households[options["household-id"]] !== role) {
    households[options["household-id"]] = role;
    log(`granting ${email} the ${role} role on ${options["household-id"]}`);
    makoJson([
      "users",
      "update-metadata",
      user.id,
      "--input",
      tempJson(`${user.id}.metadata.json`, {
        trustedMetadata: { ...trusted, households },
        profileMetadata: view.profileMetadata ?? {},
      }),
      ...tenant,
    ]);
  }
  return { id: user.id, email, role };
}

const owner = await ensureUser(options["owner-email"], options["owner-password"], "owner");
const editor = await ensureUser(options["editor-email"], options["editor-password"], "editor");

// The household and its memberships are written under a service credential
// scoped to exactly these two collections and retired right after, which is
// what the households function does for real under its own credential.
const serviceKeyId = `sk_rational_boot_${Date.now().toString(36)}`;
const serviceKey = makoJson([
  "keys",
  "service",
  "create",
  "--id",
  serviceKeyId,
  "--collection",
  "households",
  "--collection",
  "memberships",
  "--operation",
  "create",
  "--operation",
  "read",
  "--operation",
  "update",
  ...tenant,
]);
const serviceSecret = serviceKey.secret;

/**
 * Service requests name their own request id, and the data plane keys its
 * quota reservation by it, so every request needs a fresh one.
 */
function serviceHeaders() {
  return {
    "x-mako-service-key": serviceSecret,
    "x-mako-bypass-reason": "rational bootstrap seeds the demo household and its memberships",
    "x-mako-request-id": `req_${Math.random().toString(36).slice(2, 14)}${Date.now().toString(36)}`,
  };
}

async function serviceWrite(collectionId, document) {
  // The route compares the path segment with the body's id byte for byte, so
  // an id is sent verbatim (a `:` is a legal path character) rather than
  // percent-encoded.
  const path = `/service/collections/${collectionId}/documents/${document.id}`;
  const current = await dataRequest(path, { method: "GET", headers: serviceHeaders() });
  let expectedRevision = null;
  let operation = "create";
  if (current.status === 200) {
    const existing = current.body?.document ?? current.body?.body ?? null;
    expectedRevision = current.body?.revision ?? current.body?.currentRevision ?? null;
    operation = "update";
    if (existing !== null && deepEqual({ ...existing, updated_at: 0, created_at: 0 }, { ...document, updated_at: 0, created_at: 0 })) {
      log(`${collectionId}/${document.id} is already seeded`);
      return;
    }
  } else if (current.status !== 404) {
    fail(`reading ${collectionId}/${document.id} failed with ${current.status}: ${JSON.stringify(current.body)}`);
  }
  // The document route keys its idempotency by the mutation id, so the
  // header and the body carry the same value.
  const mutationId = `rational-bootstrap-${document.id}-${Date.now().toString(36)}`;
  const mutation = await dataRequest(path, {
    method: "POST",
    headers: { ...serviceHeaders(), "idempotency-key": mutationId },
    body: JSON.stringify({
      mutationId,
      operation,
      expectedRevision,
      schemaVersion: model.schemaVersion,
      body: document,
    }),
  });
  if (mutation.status !== 200 || mutation.body?.status === "conflict") {
    fail(`${operation} of ${collectionId}/${document.id} failed with ${mutation.status}: ${JSON.stringify(mutation.body)}`);
  }
  log(`${operation}d ${collectionId}/${document.id}`);
}

try {
  const at = Date.now();
  await serviceWrite("households", {
    id: options["household-id"],
    household_id: options["household-id"],
    created_at: at,
    updated_at: at,
    name: options["household-name"],
    currency: options.currency,
    owner_id: owner.id,
  });
  for (const member of [owner, editor]) {
    await serviceWrite("memberships", {
      id: `${options["household-id"]}.${member.id}`,
      household_id: options["household-id"],
      created_at: at,
      updated_at: at,
      user_id: member.id,
      email: member.email,
      role: member.role,
      status: "active",
    });
  }
} finally {
  const retired = mako(["keys", "retire", serviceKeyId, "--yes", ...tenant]);
  if (retired.status !== 0) log(`warning: service key ${serviceKeyId} could not be retired: ${retired.stderr.trim()}`);
}

console.log(
  JSON.stringify(
    {
      ...envFile,
      householdId: options["household-id"],
      owner: { email: owner.email, password: options["owner-password"], userId: owner.id },
      editor: { email: editor.email, password: options["editor-password"], userId: editor.id },
    },
    null,
    2,
  ),
);
