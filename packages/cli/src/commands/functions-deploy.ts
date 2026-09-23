import { readdir, readFile, stat } from "node:fs/promises";
import { isAbsolute, join, relative, resolve, sep } from "node:path";

import {
  type FunctionBundleArtifact,
  type FunctionBundleUploadRequest,
  type FunctionBundleUploadResult,
  type FunctionDeployment,
  ManagementApiError,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CliError, EXIT, usageError } from "../cli/errors.js";
import { CLI_NAME } from "../cli/name.js";
import type { Command, CommandArgs } from "../cli/registry.js";
import {
  CONFIGURATION_OPTIONS,
  chooseVersion,
  configurationFromArgs,
  DEPLOYMENT_OPTIONS,
  pinnedRuntimeRelease,
} from "./functions.js";
import { pairsToObject, TENANT_OPTIONS, tenantFrom } from "./shared.js";

/** The API's per-request bounds on a source bundle; checked here so a bad tree fails before upload. */
const MAX_SOURCE_FILES = 512;
const MAX_FILE_BYTES = 10 * 1024 * 1024;
const MAX_DEPENDENCIES = 256;

/** Never part of a bundle: dependency caches, version control, and dotfiles such as `.env`. */
const SKIPPED_NAMES: ReadonlySet<string> = new Set(["node_modules", ".git"]);

export interface SourceBundle {
  readonly request: FunctionBundleUploadRequest;
  readonly fileCount: number;
  readonly totalBytes: number;
}

function safeRelative(value: string, what: string): void {
  if (
    value === "" ||
    isAbsolute(value) ||
    value.includes("\\") ||
    value.split("/").some((part) => part === "" || part === "." || part === "..")
  ) {
    throw usageError(`${what} must be a safe relative path such as index.ts`);
  }
}

async function walk(root: string, directory: string, paths: string[]): Promise<void> {
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  for (const entry of entries) {
    if (SKIPPED_NAMES.has(entry.name) || entry.name.startsWith(".")) continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await walk(root, path, paths);
    } else if (entry.isFile()) {
      paths.push(relative(root, path).split(sep).join("/"));
    }
    // Symlinks, sockets, and devices are not bundled: the bundle holds only what is in the tree.
  }
}

/**
 * Reads a function directory into the source upload the console sends: every
 * regular file (dotfiles, `node_modules`, and `.git` excluded), path relative to
 * the directory with `/` separators, content base64, plus the dependency map.
 * The API archives it deterministically and returns the immutable digest.
 */
export async function buildSourceBundle(
  directory: string,
  entrypoint: string,
  dependencies: Readonly<Record<string, string>>,
): Promise<SourceBundle> {
  safeRelative(entrypoint, "--entrypoint");
  const info = await stat(directory).catch(() => undefined);
  if (info === undefined || !info.isDirectory()) {
    throw usageError(`function directory does not exist: ${directory}`);
  }
  const entry = await stat(join(directory, entrypoint)).catch(() => undefined);
  if (entry === undefined || !entry.isFile()) {
    throw usageError(`entrypoint ${entrypoint} is not a file in ${directory}`);
  }
  if (Object.keys(dependencies).length > MAX_DEPENDENCIES) {
    throw usageError(`at most ${MAX_DEPENDENCIES} --dependency mappings are allowed`);
  }
  const paths: string[] = [];
  await walk(directory, directory, paths);
  if (!paths.includes(entrypoint)) {
    throw usageError(
      `entrypoint ${entrypoint} is excluded from the bundle (hidden or skipped path)`,
    );
  }
  if (paths.length > MAX_SOURCE_FILES) {
    throw usageError(
      `${directory} holds ${paths.length} files; a bundle may hold at most ${MAX_SOURCE_FILES}`,
    );
  }
  let totalBytes = 0;
  const files = [];
  for (const path of paths) {
    const bytes = await readFile(join(directory, path));
    if (bytes.length > MAX_FILE_BYTES) {
      throw usageError(`${path} is ${bytes.length} bytes; each file may be at most 10 MiB`);
    }
    totalBytes += bytes.length;
    files.push({ path, contentBase64: bytes.toString("base64") });
  }
  return {
    request: { kind: "source", entrypoint, files, dependencies: { ...dependencies } },
    fileCount: files.length,
    totalBytes,
  };
}

function describeDiagnostic(diagnostic: FunctionBundleUploadResult["diagnostics"][number]): string {
  const where =
    diagnostic.path === undefined
      ? ""
      : ` (${diagnostic.path}${diagnostic.line === undefined ? "" : `:${diagnostic.line}`})`;
  return `${diagnostic.severity} ${diagnostic.code}: ${diagnostic.message}${where}`;
}

interface DeployReport {
  functionName: string;
  created: boolean;
  bundle?: FunctionBundleArtifact;
  version?: number;
  deployment?: FunctionDeployment;
  health?: FunctionDeployment["state"];
  promoted: boolean;
  activeVersion?: number | null;
}

async function deploy(context: CommandContext, args: CommandArgs): Promise<void> {
  const directory = resolve(context.io.cwd, args.requirePositional(0, "directory"));
  const functionName = args.requireString("name");
  if (!/^[a-z][a-z0-9-]{0,62}$/u.test(functionName)) {
    throw usageError(
      "--name must be lowercase letters, digits, and dashes, starting with a letter",
    );
  }
  const { projectId, environmentId } = tenantFrom(context, args);
  const entrypoint = args.string("entrypoint") ?? "index.ts";
  const runtimeVersion = args.string("runtime") ?? pinnedRuntimeRelease();
  const promote = !args.boolean("no-promote");
  const create = args.boolean("create");
  // Configuration options are read before any network call so a usage error costs nothing.
  const configuration =
    create && args.strings("region").length > 0 ? configurationFromArgs(args) : undefined;
  const tenant = `--project ${projectId} --env ${environmentId}`;
  const bundle = await buildSourceBundle(
    directory,
    entrypoint,
    pairsToObject(args.strings("dependency"), "--dependency"),
  );
  context.info(`bundling ${directory}: ${bundle.fileCount} files, ${bundle.totalBytes} bytes`);

  // Step lines are the command's output on a terminal; with --json they go to
  // stderr so stdout holds exactly one document.
  const step = (line: string) => {
    if (context.json) context.info(line);
    else context.io.stdout.write(`${line}\n`);
  };
  const report: DeployReport = { functionName, created: false, promoted: false };
  const client = await context.management();

  try {
    await client.getFunction(projectId, environmentId, functionName);
  } catch (error) {
    if (!(error instanceof ManagementApiError && error.status === 404)) throw error;
    if (!create) {
      throw new CliError(
        `function ${functionName} does not exist; pass --create with --region <region> to create it, or run ${CLI_NAME} functions create ${functionName} ${tenant}`,
        EXIT.notFound,
        "CLI_FUNCTION_MISSING",
      );
    }
    if (configuration === undefined) {
      throw usageError("--create needs --region <region> (and any other configuration options)");
    }
    await client.createFunction(
      projectId,
      environmentId,
      { name: functionName, configuration },
      context.idempotencyKey(),
    );
    report.created = true;
    step(`function ${functionName}`);
  }

  const upload = await client.uploadFunctionBundle(
    projectId,
    environmentId,
    bundle.request,
    context.idempotencyKey(),
  );
  for (const diagnostic of upload.diagnostics) context.info(describeDiagnostic(diagnostic));
  const artifact = upload.artifact;
  if (upload.status !== "ready" || artifact === undefined) {
    throw new CliError(
      `the bundle was rejected with ${upload.diagnostics.length} diagnostic(s); fix the source and run the deploy again`,
      EXIT.refused,
      "CLI_BUNDLE_REJECTED",
    );
  }
  report.bundle = artifact;
  step(`bundle ${artifact.digest}`);
  context.info(
    `resume: ${CLI_NAME} functions deployments create ${functionName} --bundle ${artifact.digest} --entrypoint ${artifact.entrypoint} --runtime ${runtimeVersion} ${tenant}`,
  );

  const version = await chooseVersion(client, projectId, environmentId, functionName, args);
  const deployment = await client.createFunctionDeployment(
    projectId,
    environmentId,
    functionName,
    { version, bundleDigest: artifact.digest, entrypoint: artifact.entrypoint, runtimeVersion },
    context.idempotencyKey(),
  );
  report.version = deployment.version;
  report.deployment = deployment;
  step(`version ${deployment.version}`);
  context.info(
    `resume: ${CLI_NAME} functions deployments health ${functionName} ${deployment.version} ${tenant}`,
  );

  const checked = await client.checkFunctionDeploymentHealth(
    projectId,
    environmentId,
    functionName,
    deployment.version,
  );
  report.deployment = checked;
  report.health = checked.state;
  step(`health ${checked.state}`);
  if (checked.diagnostic) context.info(`diagnostic: ${checked.diagnostic}`);
  if (checked.state !== "healthy") {
    context.info(
      `resume: ${CLI_NAME} functions deployments health ${functionName} ${deployment.version} ${tenant}`,
    );
    throw new CliError(
      `version ${deployment.version} of ${functionName} is ${checked.state}; it was not promoted`,
      EXIT.refused,
      "CLI_DEPLOYMENT_UNHEALTHY",
    );
  }
  context.info(
    `resume: ${CLI_NAME} functions deployments promote ${functionName} ${deployment.version} ${tenant} --yes`,
  );

  if (promote) {
    // Promotion changes what every caller of the function runs: confirmed like
    // `functions deployments promote`, and skipped entirely with --no-promote.
    await context.confirmDestructive("promote deployment", `${functionName}@${deployment.version}`);
    const promoted = await client.promoteFunctionDeployment(
      projectId,
      environmentId,
      functionName,
      deployment.version,
      context.idempotencyKey(),
    );
    report.promoted = true;
    report.activeVersion = promoted.activeVersion;
    step(`active ${promoted.activeVersion ?? "none"}`);
  } else {
    context.info(`not promoted (--no-promote); version ${deployment.version} is ready`);
  }
  if (context.json) context.out(report);
}

export const functionsDeployCommands: readonly Command[] = [
  {
    path: ["functions", "deploy"],
    summary:
      "Upload a function directory, create a version, check its health, and promote it (promotion needs --yes or a typed confirmation)",
    operations: [
      "getFunction",
      "createFunction",
      "uploadFunctionBundle",
      "listFunctionDeployments",
      "createFunctionDeployment",
      "checkFunctionDeploymentHealth",
      "promoteFunctionDeployment",
    ],
    positionals: [{ name: "directory", description: "Function source directory", required: true }],
    options: {
      ...TENANT_OPTIONS,
      name: {
        type: "string",
        short: "n",
        required: true,
        description: "Function name to deploy to",
        placeholder: "<function-name>",
      },
      ...DEPLOYMENT_OPTIONS,
      dependency: {
        type: "string",
        multiple: true,
        description: "Map an import specifier to an uploaded path, as specifier=path (repeatable)",
        placeholder: "<specifier=path>",
      },
      "no-promote": {
        type: "boolean",
        description:
          "Stop after the health check; promote later with `functions deployments promote`",
      },
      create: {
        type: "boolean",
        description: "Create the function first when it does not exist (needs --region)",
      },
      ...CONFIGURATION_OPTIONS,
    },
    run: deploy,
  },
];
