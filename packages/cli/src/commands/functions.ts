import { readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import {
  type CreateFunctionRequest,
  type FunctionConfiguration,
  type FunctionDeployment,
  type FunctionLogPage,
  type FunctionSecret,
  type FunctionTestRequest,
  ManagementApiError,
  type MakoManagementClient,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CliError, EXIT, usageError } from "../cli/errors.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import {
  pairsToObject,
  SECRET_FILE_OPTION,
  type Tenant,
  TENANT_OPTIONS,
  tenantFrom,
} from "./shared.js";

// ---- shared pieces (also used by `functions deploy`) ------------------------

const NAME: PositionalSpec = { name: "name", description: "Function name", required: true };
const VERSION: PositionalSpec = {
  name: "version",
  description: "Deployment version number",
  required: true,
};

/** Default limits: the ones the local bootstrap deploys with, all within the API's bounds. */
const DEFAULT_LIMITS = {
  cpuMilliseconds: 1_000,
  wallMilliseconds: 10_000,
  memoryBytes: 128 * 1024 * 1024,
  requestBytes: 1024 * 1024,
  responseBytes: 1024 * 1024,
  concurrency: 4,
} as const;

/** Options that describe a `FunctionConfiguration` without a JSON document. */
export const CONFIGURATION_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  region: {
    type: "string",
    multiple: true,
    description: "Region the function runs in (repeatable)",
    placeholder: "<region>",
  },
  secret: {
    type: "string",
    multiple: true,
    description: "Function secret name to inject (repeatable)",
    placeholder: "<secret-name>",
  },
  "no-verify-jwt": {
    type: "boolean",
    description: "Allow public invocation without an application JWT",
  },
  "cpu-ms": {
    type: "string",
    description: `CPU time limit in milliseconds (default: ${DEFAULT_LIMITS.cpuMilliseconds})`,
    placeholder: "<n>",
  },
  "wall-ms": {
    type: "string",
    description: `Wall time limit in milliseconds (default: ${DEFAULT_LIMITS.wallMilliseconds})`,
    placeholder: "<n>",
  },
  "memory-bytes": {
    type: "string",
    description: `Memory limit in bytes (default: ${DEFAULT_LIMITS.memoryBytes})`,
    placeholder: "<n>",
  },
  "request-bytes": {
    type: "string",
    description: `Request size limit in bytes (default: ${DEFAULT_LIMITS.requestBytes})`,
    placeholder: "<n>",
  },
  "response-bytes": {
    type: "string",
    description: `Response size limit in bytes (default: ${DEFAULT_LIMITS.responseBytes})`,
    placeholder: "<n>",
  },
  concurrency: {
    type: "string",
    description: `Concurrent invocations per worker (default: ${DEFAULT_LIMITS.concurrency})`,
    placeholder: "<n>",
  },
};

export function positiveInteger(args: CommandArgs, name: string, fallback: number): number {
  const value = args.integer(name) ?? fallback;
  if (value < 1) throw usageError(`--${name} must be a positive integer`);
  return value;
}

export function versionPositional(args: CommandArgs, index: number): number {
  const raw = args.requirePositional(index, "version");
  if (!/^\d+$/u.test(raw) || Number.parseInt(raw, 10) < 1) {
    throw usageError("<version> must be a positive integer");
  }
  return Number.parseInt(raw, 10);
}

/** Builds a configuration from `CONFIGURATION_OPTIONS`; `--region` is required. */
export function configurationFromArgs(args: CommandArgs): FunctionConfiguration {
  const regions = [...new Set(args.strings("region"))];
  if (regions.length === 0) throw usageError("--region <region> is required at least once");
  return {
    verifyJwt: !args.boolean("no-verify-jwt"),
    regions,
    secretNames: [...new Set(args.strings("secret"))],
    limits: {
      cpuMilliseconds: positiveInteger(args, "cpu-ms", DEFAULT_LIMITS.cpuMilliseconds),
      wallMilliseconds: positiveInteger(args, "wall-ms", DEFAULT_LIMITS.wallMilliseconds),
      memoryBytes: positiveInteger(args, "memory-bytes", DEFAULT_LIMITS.memoryBytes),
      requestBytes: positiveInteger(args, "request-bytes", DEFAULT_LIMITS.requestBytes),
      responseBytes: positiveInteger(args, "response-bytes", DEFAULT_LIMITS.responseBytes),
      concurrency: positiveInteger(args, "concurrency", DEFAULT_LIMITS.concurrency),
    },
  };
}

function isConfiguration(value: unknown): value is FunctionConfiguration {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { verifyJwt?: unknown }).verifyJwt === "boolean" &&
    Array.isArray((value as { regions?: unknown }).regions) &&
    typeof (value as { limits?: unknown }).limits === "object"
  );
}

/**
 * The runtime release a deployment pins by default: the same one `functions
 * serve` runs, read from the bundled pin so the two never drift.
 */
export function pinnedRuntimeRelease(): string {
  const path = fileURLToPath(new URL("../../runtime/runtime-pin.json", import.meta.url));
  const pin = JSON.parse(readFileSync(path, "utf8")) as { release?: unknown };
  if (typeof pin.release !== "string" || pin.release === "") {
    throw usageError("the bundled runtime pin has no release; pass --runtime <release>");
  }
  return pin.release;
}

export const DEPLOYMENT_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  entrypoint: {
    type: "string",
    description: "Entrypoint path inside the bundle (default: index.ts)",
    placeholder: "<path>",
  },
  runtime: {
    type: "string",
    description: "Runtime release to pin (default: the release functions serve uses)",
    placeholder: "<release>",
  },
  version: {
    type: "string",
    description: "Version number for the new deployment (default: highest existing + 1)",
    placeholder: "<n>",
  },
};

/** The explicit `--version`, or one past the highest deployed version. */
export async function chooseVersion(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  functionName: string,
  args: CommandArgs,
): Promise<number> {
  const explicit = args.integer("version");
  if (explicit !== undefined) {
    if (explicit < 1) throw usageError("--version must be a positive integer");
    return explicit;
  }
  const deployments = await client.listFunctionDeployments(projectId, environmentId, functionName);
  return deployments.reduce((highest, item) => Math.max(highest, item.version), 0) + 1;
}

const FUNCTION_COLUMNS = [
  { key: "name" },
  { key: "state" },
  { key: "activeVersion" },
  { key: "updatedAt" },
];

const DEPLOYMENT_COLUMNS = [
  { key: "version" },
  { key: "state" },
  { key: "bundleDigest" },
  { key: "entrypoint" },
  { key: "runtimeVersion" },
  { key: "createdAt" },
];

function logLine(item: unknown): string {
  const entry = item as FunctionLogPage["items"][number];
  return `${entry.timestamp} ${entry.level} v${entry.version} ${entry.region} ${entry.correlationId} ${entry.message}`;
}

function decodeBody(base64: string): string {
  const bytes = Buffer.from(base64, "base64");
  const text = bytes.toString("utf8");
  return Buffer.from(text, "utf8").equals(bytes) ? text : `(binary, ${bytes.length} bytes)`;
}

// ---- functions ------------------------------------------------------------

async function list(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.listFunctions(projectId, environmentId), {
    columns: FUNCTION_COLUMNS,
  });
}

async function create(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const source = args.string("input");
  let request: CreateFunctionRequest;
  if (source !== undefined) {
    const document = await context.readJson<Record<string, unknown>>(source, "--input");
    const configuration = "configuration" in document ? document.configuration : document;
    if (!isConfiguration(configuration)) {
      throw usageError(
        "--input must be a CreateFunctionRequest or a FunctionConfiguration (verifyJwt, regions, secretNames, limits)",
      );
    }
    if (typeof document.name === "string" && document.name !== name) {
      throw usageError(`--input names "${document.name}" but <name> is "${name}"`);
    }
    request = { name, configuration };
  } else {
    request = { name, configuration: configurationFromArgs(args) };
  }
  const client = await context.management();
  context.out(
    await client.createFunction(projectId, environmentId, request, context.idempotencyKey()),
  );
}

async function get(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getFunction(projectId, environmentId, name));
}

async function update(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const configuration = await context.readJson<unknown>(args.requireString("config"), "--config");
  if (!isConfiguration(configuration)) {
    throw usageError(
      "--config must be a FunctionConfiguration (verifyJwt, regions, secretNames, limits)",
    );
  }
  const client = await context.management();
  context.out(
    await client.updateFunctionConfiguration(projectId, environmentId, name, configuration),
  );
}

async function remove(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.deleteFunction(projectId, environmentId, name));
}

// ---- deployments ----------------------------------------------------------

async function deploymentsList(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.listFunctionDeployments(projectId, environmentId, name), {
    columns: DEPLOYMENT_COLUMNS,
  });
}

async function deploymentsCreate(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const bundleDigest = args.requireString("bundle");
  const client = await context.management();
  const version = await chooseVersion(client, projectId, environmentId, name, args);
  const deployment = await client.createFunctionDeployment(
    projectId,
    environmentId,
    name,
    {
      version,
      bundleDigest,
      entrypoint: args.string("entrypoint") ?? "index.ts",
      runtimeVersion: args.string("runtime") ?? pinnedRuntimeRelease(),
    },
    context.idempotencyKey(),
  );
  context.out(deployment);
}

async function deploymentsGet(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const version = versionPositional(args, 1);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getFunctionDeployment(projectId, environmentId, name, version));
}

async function deploymentsDelete(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const version = versionPositional(args, 1);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  await client.deleteFunctionDeployment(projectId, environmentId, name, version);
  if (context.json) context.out({ functionName: name, version, deleted: true });
  else context.info(`Deleted ${name} version ${version}.`);
}

async function deploymentsPromote(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const version = versionPositional(args, 1);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.promoteFunctionDeployment(
      projectId,
      environmentId,
      name,
      version,
      context.idempotencyKey(),
    ),
  );
}

async function deploymentsRollback(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const version = versionPositional(args, 1);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.rollbackFunctionDeployment(
      projectId,
      environmentId,
      name,
      version,
      context.idempotencyKey(),
    ),
  );
}

async function deploymentsHealth(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const version = versionPositional(args, 1);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const deployment: FunctionDeployment = await client.checkFunctionDeploymentHealth(
    projectId,
    environmentId,
    name,
    version,
  );
  context.out(deployment);
}

// ---- test and logs --------------------------------------------------------

async function testInvocation(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const bodySource = args.string("body");
  const body = bodySource === undefined ? "" : await context.readInput(bodySource);
  const version = args.integer("version");
  if (version !== undefined && version < 1)
    throw usageError("--version must be a positive integer");
  const request: FunctionTestRequest = {
    ...(version === undefined ? {} : { version }),
    method: (args.string("method") ?? "GET").toUpperCase(),
    path: args.string("path") ?? "/",
    headers: pairsToObject(args.strings("header"), "--header"),
    body: Buffer.from(body, "utf8").toString("base64"),
  };
  const client = await context.management();
  const response = await client.testFunctionInvocation(projectId, environmentId, name, request);
  if (context.json) {
    context.out(response);
    return;
  }
  context.out({
    status: response.status,
    correlationId: response.correlationId,
    headers: response.headers,
    body: decodeBody(response.body),
  });
}

async function logs(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const limit = args.integer("limit");
  if (limit !== undefined && (limit < 1 || limit > 1000)) {
    throw usageError("--limit must be between 1 and 1000");
  }
  const start = args.string("cursor");
  const client = await context.management();
  const page = await context.collect((cursor) => {
    const from = cursor ?? start;
    return client.queryFunctionLogs(projectId, environmentId, name, {
      ...(from === undefined ? {} : { cursor: from }),
      ...(limit === undefined ? {} : { limit }),
    });
  });
  context.out(page, { line: logLine });
}

// ---- secrets --------------------------------------------------------------

/** The options that carry a value the caller already holds, if either is set. */
const SUPPLIED_VALUE_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  value: {
    type: "string",
    description:
      "Store this value instead of a generated one (a scoped service credential, say); it is never shown again",
    placeholder: "<value>",
  },
  "value-file": {
    type: "string",
    description:
      "Read the value to store from this file, ignoring one trailing newline; safer than --value, which a shell records",
    placeholder: "<path>",
  },
};

async function suppliedValue(args: CommandArgs): Promise<string | undefined> {
  const inline = args.string("value");
  const path = args.string("value-file");
  if (inline !== undefined && path !== undefined) {
    throw usageError("pass either --value or --value-file, not both");
  }
  if (inline !== undefined) {
    if (inline === "") throw usageError("--value cannot be empty");
    return inline;
  }
  if (path === undefined) return undefined;
  const contents = await readFile(path, "utf8").catch(() => {
    throw usageError(`--value-file ${path} could not be read`);
  });
  // `--secret-file` writes a value with a trailing newline, so a value read
  // back from one round-trips without the caller having to trim it.
  const value = contents.replace(/\r?\n$/u, "");
  if (value === "") throw usageError(`--value-file ${path} is empty`);
  return value;
}

/**
 * Creates a secret from a value the caller supplies.
 *
 * `PUT …/function-secrets/{secretName}` has no management-SDK method, so --
 * like the data-job artifact transfer in `data.ts` -- the request is made
 * directly with the same bearer credential and Origin the SDK would send. The
 * value goes in the body and is never echoed back.
 */
async function putSecretValue(
  context: CommandContext,
  tenant: Tenant,
  name: string,
  value: string,
): Promise<FunctionSecret> {
  return await secretValueRequest(
    context,
    `v1/projects/${encodeURIComponent(tenant.projectId)}/environments/${encodeURIComponent(
      tenant.environmentId,
    )}/function-secrets/${encodeURIComponent(name)}`,
    "PUT",
    value,
    context.idempotencyKey(),
  );
}

/** One request carrying a secret value: creation puts, rotation posts. */
async function secretValueRequest(
  context: CommandContext,
  path: string,
  method: "PUT" | "POST",
  value: string,
  idempotencyKey: string,
): Promise<FunctionSecret> {
  const endpoint = await context.endpoint();
  const credential = await context.credential();
  const provider = credential.accessToken;
  const token = typeof provider === "string" ? provider : await provider();
  const url = new URL(path, endpoint.endsWith("/") ? endpoint : `${endpoint}/`);
  const fetchImpl = context.io.fetch ?? globalThis.fetch;
  const response = await fetchImpl(
    new Request(url, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/json",
        "Content-Type": "application/json",
        "Idempotency-Key": idempotencyKey,
        Origin: new URL(endpoint).origin,
      },
      body: JSON.stringify({ value }),
    }),
  );
  if (!response.ok) throw await secretValueError(response);
  return (await response.json()) as FunctionSecret;
}

async function secretValueError(response: Response): Promise<Error> {
  const body: unknown = await response.json().catch(() => undefined);
  const error = isRecord(body) ? body.error : undefined;
  if (isRecord(error) && typeof error.code === "string" && typeof error.message === "string") {
    return new ManagementApiError(
      error as unknown as ConstructorParameters<typeof ManagementApiError>[0],
      response.status,
    );
  }
  return new CliError(
    `storing the function secret failed with HTTP ${response.status}`,
    EXIT.api,
    "CLI_SECRET_VALUE",
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Rotate a secret to a value the caller supplies. The management SDK has no
 * method for the body-carrying form of this route yet, so the request is made
 * directly, as `putSecretValue` does for creation.
 */
async function rotateSecretValue(
  context: CommandContext,
  tenant: Tenant,
  name: string,
  value: string,
): Promise<FunctionSecret> {
  return await secretValueRequest(
    context,
    `v1/projects/${encodeURIComponent(tenant.projectId)}/environments/${encodeURIComponent(
      tenant.environmentId,
    )}/function-secrets/${encodeURIComponent(name)}/actions/rotate`,
    "POST",
    value,
    context.idempotencyKey(),
  );
}

async function secretsCreate(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const tenant = tenantFrom(context, args);
  const value = await suppliedValue(args);
  if (value !== undefined) {
    // The caller already has the value, so nothing is displayed and
    // --secret-file would have nothing to write; silently ignoring it would
    // leave someone believing a file holds the secret.
    if (args.string("secret-file") !== undefined) {
      throw usageError("--secret-file has nothing to write for a supplied value");
    }
    context.out(await putSecretValue(context, tenant, name, value));
    return;
  }
  const client = await context.management();
  const issue = await client.createFunctionSecret(
    tenant.projectId,
    tenant.environmentId,
    name,
    context.idempotencyKey(),
  );
  await context.secret(
    "function secret value",
    issue.value,
    issue.secret,
    args.string("secret-file"),
  );
}

async function secretsGet(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getFunctionSecret(projectId, environmentId, name));
}

async function secretsRetire(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.retireFunctionSecret(projectId, environmentId, name));
}

async function secretsRotate(context: CommandContext, args: CommandArgs): Promise<void> {
  const name = args.requirePositional(0, "name");
  const { projectId, environmentId } = tenantFrom(context, args);
  const value = await suppliedValue(args);
  if (value !== undefined) {
    // As on creation: the caller already holds it, so nothing is displayed and
    // --secret-file would have nothing to write.
    if (args.string("secret-file") !== undefined) {
      throw usageError("--secret-file has nothing to write for a supplied value");
    }
    context.out(await rotateSecretValue(context, { projectId, environmentId }, name, value));
    return;
  }
  const client = await context.management();
  const issue = await client.rotateFunctionSecret(
    projectId,
    environmentId,
    name,
    context.idempotencyKey(),
  );
  await context.secret(
    "function secret value",
    issue.value,
    issue.secret,
    args.string("secret-file"),
  );
}

// ---- registry -------------------------------------------------------------

const versioned = (args: CommandArgs) =>
  `${args.requirePositional(0, "name")}@${args.requirePositional(1, "version")}`;

export const functionsCommands: readonly Command[] = [
  {
    path: ["functions", "list"],
    summary: "List the functions in an environment",
    operations: ["listFunctions"],
    options: TENANT_OPTIONS,
    run: list,
  },
  {
    path: ["functions", "create"],
    summary: "Create a function with its configuration; deploy code with `functions deploy`",
    operations: ["createFunction"],
    positionals: [NAME],
    options: {
      ...TENANT_OPTIONS,
      input: {
        type: "string",
        description: "CreateFunctionRequest or FunctionConfiguration JSON: -, @path, or inline",
        placeholder: "<json>",
      },
      ...CONFIGURATION_OPTIONS,
    },
    run: create,
  },
  {
    path: ["functions", "get"],
    summary: "Show a function, its configuration, and its active version",
    operations: ["getFunction"],
    positionals: [NAME],
    options: TENANT_OPTIONS,
    run: get,
  },
  {
    path: ["functions", "update"],
    summary: "Replace a function's configuration",
    operations: ["updateFunctionConfiguration"],
    positionals: [NAME],
    options: {
      ...TENANT_OPTIONS,
      config: {
        type: "string",
        required: true,
        description: "FunctionConfiguration JSON: -, @path, or inline",
        placeholder: "<json>",
      },
    },
    run: update,
  },
  {
    path: ["functions", "delete"],
    summary: "Delete a function and every deployment it has",
    operations: ["deleteFunction"],
    positionals: [NAME],
    options: TENANT_OPTIONS,
    destructive: {
      action: "delete function",
      resource: (args) => args.requirePositional(0, "name"),
    },
    run: remove,
  },
  {
    path: ["functions", "deployments", "list"],
    summary: "List a function's immutable deployment versions",
    operations: ["listFunctionDeployments"],
    positionals: [NAME],
    options: TENANT_OPTIONS,
    run: deploymentsList,
  },
  {
    path: ["functions", "deployments", "create"],
    summary: "Create a deployment version from an uploaded bundle digest",
    operations: ["createFunctionDeployment", "listFunctionDeployments"],
    positionals: [NAME],
    options: {
      ...TENANT_OPTIONS,
      bundle: {
        type: "string",
        required: true,
        description: "Bundle digest printed by `functions deploy` (sha256:…)",
        placeholder: "<digest>",
      },
      ...DEPLOYMENT_OPTIONS,
    },
    run: deploymentsCreate,
  },
  {
    path: ["functions", "deployments", "get"],
    summary: "Show one deployment version",
    operations: ["getFunctionDeployment"],
    positionals: [NAME, VERSION],
    options: TENANT_OPTIONS,
    run: deploymentsGet,
  },
  {
    path: ["functions", "deployments", "delete"],
    summary: "Delete a deployment version that is not active",
    operations: ["deleteFunctionDeployment"],
    positionals: [NAME, VERSION],
    options: TENANT_OPTIONS,
    destructive: { action: "delete deployment", resource: versioned },
    run: deploymentsDelete,
  },
  {
    path: ["functions", "deployments", "promote"],
    summary: "Make a healthy deployment version the active one",
    operations: ["promoteFunctionDeployment"],
    positionals: [NAME, VERSION],
    options: TENANT_OPTIONS,
    destructive: { action: "promote deployment", resource: versioned },
    run: deploymentsPromote,
  },
  {
    path: ["functions", "deployments", "rollback"],
    summary: "Switch the active version back to a previously healthy one",
    operations: ["rollbackFunctionDeployment"],
    positionals: [NAME, VERSION],
    options: TENANT_OPTIONS,
    destructive: { action: "roll back to deployment", resource: versioned },
    run: deploymentsRollback,
  },
  {
    path: ["functions", "deployments", "health"],
    summary: "Run the health check on a deployment version and record the outcome",
    operations: ["checkFunctionDeploymentHealth"],
    positionals: [NAME, VERSION],
    options: TENANT_OPTIONS,
    run: deploymentsHealth,
  },
  {
    path: ["functions", "test"],
    summary: "Invoke a function through the management test route and show the response",
    operations: ["testFunctionInvocation"],
    positionals: [NAME],
    options: {
      ...TENANT_OPTIONS,
      method: {
        type: "string",
        short: "X",
        description: "HTTP method (default: GET)",
        placeholder: "<method>",
      },
      path: { type: "string", description: "Request path (default: /)", placeholder: "<path>" },
      body: {
        type: "string",
        description: "Request body: -, @path, or inline text",
        placeholder: "<body>",
      },
      header: {
        type: "string",
        short: "H",
        multiple: true,
        description: "Request header as name=value (repeatable)",
        placeholder: "<name=value>",
      },
      version: {
        type: "string",
        description: "Deployment version to invoke (default: the active one)",
        placeholder: "<n>",
      },
    },
    run: testInvocation,
  },
  {
    path: ["functions", "logs"],
    summary: "Read a function's sanitized logs, newest page first",
    operations: ["queryFunctionLogs"],
    positionals: [NAME],
    options: {
      ...TENANT_OPTIONS,
      limit: {
        type: "string",
        description: "Entries per page, 1-1000 (default: 100)",
        placeholder: "<n>",
      },
      cursor: {
        type: "string",
        description: "Continue from a cursor printed by a previous page",
        placeholder: "<cursor>",
      },
    },
    run: logs,
  },
  {
    path: ["functions", "secrets", "create"],
    summary:
      "Create a function secret, either generated and shown once or from a value you supply with --value/--value-file",
    operations: ["createFunctionSecret", "createFunctionSecretValue"],
    positionals: [{ name: "name", description: "Secret name", required: true }],
    options: { ...TENANT_OPTIONS, ...SECRET_FILE_OPTION, ...SUPPLIED_VALUE_OPTIONS },
    run: secretsCreate,
  },
  {
    path: ["functions", "secrets", "get"],
    summary: "Show a function secret's version and state, never its value",
    operations: ["getFunctionSecret"],
    positionals: [{ name: "name", description: "Secret name", required: true }],
    options: TENANT_OPTIONS,
    run: secretsGet,
  },
  {
    path: ["functions", "secrets", "retire"],
    summary: "Retire a function secret so no new deployment can attach it",
    operations: ["retireFunctionSecret"],
    positionals: [{ name: "name", description: "Secret name", required: true }],
    options: TENANT_OPTIONS,
    destructive: {
      action: "retire secret",
      resource: (args) => args.requirePositional(0, "name"),
    },
    run: secretsRetire,
  },
  {
    path: ["functions", "secrets", "rotate"],
    summary: "Rotate a function secret, to a generated value or one you supply",
    operations: ["rotateFunctionSecret"],
    positionals: [{ name: "name", description: "Secret name", required: true }],
    options: { ...TENANT_OPTIONS, ...SECRET_FILE_OPTION, ...SUPPLIED_VALUE_OPTIONS },
    destructive: {
      action: "rotate secret",
      resource: (args) => args.requirePositional(0, "name"),
    },
    run: secretsRotate,
  },
];
