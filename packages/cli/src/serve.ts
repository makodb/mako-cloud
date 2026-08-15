import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { accessSync, constants as fsConstants, readFileSync, statSync } from "node:fs";
import { basename, delimiter, isAbsolute, join, resolve } from "node:path";
import type { Readable, Writable } from "node:stream";
import { fileURLToPath } from "node:url";

interface RuntimePin {
  readonly provider: "supabase-edge-runtime";
  readonly release: string;
  readonly imageRepository: string;
  readonly imageDigest: string;
  readonly protocolVersion: number;
}

// Let the supervised function worker enforce the public wall limit and return
// its bounded error before the outer edge-runtime process can time out the
// main supervisor request.
const RUNTIME_SUPERVISOR_GRACE_MILLISECONDS = 30_000;

export interface LocalServeConfig {
  readonly functionDirectory: string;
  readonly functionName: string;
  readonly entrypoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly apiUrl: string;
  readonly port: number;
  readonly wallTimeMilliseconds: number;
  readonly verifyJwt: boolean;
  readonly jwksFile?: string;
  readonly jwtIssuer?: string;
  readonly jwtAudience?: string;
  readonly envFiles: readonly string[];
  readonly secretFiles: readonly string[];
  readonly containerEngine?: "docker" | "podman";
  readonly dryRun: boolean;
}

export interface RuntimeLaunchPlan {
  readonly command: "docker" | "podman";
  readonly args: readonly string[];
  readonly environment: Readonly<Record<string, string>>;
  readonly image: string;
  readonly exposedEnvironmentNames: readonly string[];
  readonly secretNames: readonly string[];
}

export class LocalServeConfigurationError extends Error {
  override readonly name: string = "LocalServeConfigurationError";
}

export function parseServeArguments(args: readonly string[], cwd: string): LocalServeConfig {
  if (args[0] !== "functions" || args[1] !== "serve") {
    throw new LocalServeConfigurationError(
      "usage: mako functions serve <directory> --project-id <id> --environment-id <id> [options]",
    );
  }
  const directoryArgument = args[2];
  if (directoryArgument === undefined || directoryArgument.startsWith("--")) {
    throw new LocalServeConfigurationError("a function directory is required");
  }
  const options = new Map<string, string[]>();
  let verifyJwt = true;
  let dryRun = false;
  for (let index = 3; index < args.length; index += 1) {
    const flag = args[index];
    if (flag === "--no-verify-jwt") {
      verifyJwt = false;
      continue;
    }
    if (flag === "--verify-jwt") {
      verifyJwt = true;
      continue;
    }
    if (flag === "--dry-run") {
      dryRun = true;
      continue;
    }
    if (flag === undefined || !flag.startsWith("--")) {
      throw new LocalServeConfigurationError(`unexpected argument: ${flag ?? ""}`);
    }
    const value = args[index + 1];
    if (value === undefined || value.startsWith("--")) {
      throw new LocalServeConfigurationError(`${flag} requires a value`);
    }
    const values = options.get(flag) ?? [];
    values.push(value);
    options.set(flag, values);
    index += 1;
  }
  const functionDirectory = absolutePath(directoryArgument, cwd);
  requireDirectory(functionDirectory, "function directory");
  const entrypoint = one(options, "--entrypoint") ?? "index.ts";
  validateRelativeEntrypoint(entrypoint);
  requireFile(join(functionDirectory, entrypoint), "function entrypoint");
  const projectId = required(options, "--project-id");
  const environmentId = required(options, "--environment-id");
  const functionName = one(options, "--function-name") ?? basename(functionDirectory);
  validateIdentifier(projectId, /^prj_[A-Za-z0-9_-]{8,64}$/u, "project ID");
  validateIdentifier(environmentId, /^env_[A-Za-z0-9_-]{8,64}$/u, "environment ID");
  validateIdentifier(functionName, /^[a-z][a-z0-9-]{0,62}$/u, "function name");
  const port = Number(one(options, "--port") ?? "9000");
  if (!Number.isSafeInteger(port) || port < 1_024 || port > 65_535) {
    throw new LocalServeConfigurationError("port must be between 1024 and 65535");
  }
  const wallTimeMilliseconds = Number(one(options, "--wall-time-ms") ?? "300000");
  if (
    !Number.isSafeInteger(wallTimeMilliseconds) ||
    wallTimeMilliseconds < 100 ||
    wallTimeMilliseconds > 300_000
  ) {
    throw new LocalServeConfigurationError("wall time must be between 100 and 300000 milliseconds");
  }
  const apiUrl = validateApiUrl(one(options, "--api-url") ?? "http://host.docker.internal:8787");
  const jwksFile = optionalFile(options, "--jwks-file", cwd);
  const jwtIssuer = one(options, "--jwt-issuer");
  const jwtAudience = one(options, "--jwt-audience");
  const suppliedJwtSettings = [jwksFile, jwtIssuer, jwtAudience].filter(
    (value) => value !== undefined,
  ).length;
  if (suppliedJwtSettings > 0 && suppliedJwtSettings < 3) {
    throw new LocalServeConfigurationError(
      "JWT configuration requires --jwks-file, --jwt-issuer, and --jwt-audience together",
    );
  }
  if (
    verifyJwt &&
    (jwksFile === undefined || jwtIssuer === undefined || jwtAudience === undefined)
  ) {
    throw new LocalServeConfigurationError(
      "JWT verification requires --jwks-file, --jwt-issuer, and --jwt-audience",
    );
  }
  const engine = one(options, "--container-engine");
  if (engine !== undefined && engine !== "docker" && engine !== "podman") {
    throw new LocalServeConfigurationError("container engine must be docker or podman");
  }
  rejectUnknownOptions(options);
  return {
    functionDirectory,
    functionName,
    entrypoint,
    projectId,
    environmentId,
    apiUrl,
    port,
    wallTimeMilliseconds,
    verifyJwt,
    ...(jwksFile === undefined ? {} : { jwksFile }),
    ...(jwtIssuer === undefined ? {} : { jwtIssuer }),
    ...(jwtAudience === undefined ? {} : { jwtAudience }),
    envFiles: manyFiles(options, "--env-file", cwd),
    secretFiles: manyFiles(options, "--secret-file", cwd),
    ...(engine === undefined ? {} : { containerEngine: engine }),
    dryRun,
  };
}

export function createRuntimeLaunchPlan(config: LocalServeConfig): RuntimeLaunchPlan {
  const pin = runtimePin();
  const engine = config.containerEngine ?? findContainerEngine();
  const ordinary = readEnvironmentFiles(config.envFiles);
  const secrets = readEnvironmentFiles(config.secretFiles);
  for (const [name, value] of Object.entries(secrets)) {
    if (value.length < 1 || value.length > 64 * 1024) {
      throw new LocalServeConfigurationError(
        `secret ${name} must contain between 1 and 65536 bytes`,
      );
    }
  }
  for (const name of Object.keys(secrets)) {
    if (Object.hasOwn(ordinary, name)) {
      throw new LocalServeConfigurationError(`environment name is duplicated: ${name}`);
    }
  }
  const exposedEnvironmentNames = [...Object.keys(ordinary), ...Object.keys(secrets)].sort();
  const runtimeEnvironment: Record<string, string> = {
    ...ordinary,
    ...secrets,
    DENO_DIR: "/tmp/deno-cache",
    MAKO_API_URL: config.apiUrl,
    MAKO_ENTRYPOINT: config.entrypoint,
    MAKO_ENVIRONMENT_ID: config.environmentId,
    MAKO_FUNCTION_NAME: config.functionName,
    MAKO_FUNCTION_PATH: "/home/deno/functions/user",
    MAKO_JWT_AUDIENCE: config.jwtAudience ?? "",
    MAKO_JWT_ISSUER: config.jwtIssuer ?? "",
    MAKO_JWKS: config.jwksFile === undefined ? "" : readAndValidateJwks(config.jwksFile),
    MAKO_PROJECT_ID: config.projectId,
    MAKO_RUNTIME_AUTHORIZATION: randomBytes(32).toString("hex"),
    MAKO_RUNTIME_REGION: "local",
    MAKO_RUNTIME_STATE_KEY: randomBytes(32).toString("hex"),
    MAKO_RUNTIME_STATE_PATH: "/tmp/mako-runtime-supervisor-state",
    MAKO_RUNTIME_WORKER_PATH: "/tmp/mako-runtime-workers",
    MAKO_USER_ENV_NAMES: JSON.stringify(exposedEnvironmentNames),
    MAKO_VERIFY_JWT: String(config.verifyJwt),
    MAKO_WALL_TIME_MS: String(config.wallTimeMilliseconds),
    EDGE_RUNTIME_PORT: "9000",
  };
  const runtimeDirectory = fileURLToPath(new URL("../runtime/main", import.meta.url));
  const image = `${pin.imageRepository}@${pin.imageDigest}`;
  const args = [
    "run",
    "--rm",
    "--init",
    "--read-only",
    "--publish",
    `127.0.0.1:${config.port}:9000`,
    "--add-host",
    "host.docker.internal:host-gateway",
    "--mount",
    `type=bind,src=${config.functionDirectory},dst=/home/deno/functions/user,readonly`,
    "--mount",
    `type=bind,src=${runtimeDirectory},dst=/home/deno/functions/main,readonly`,
    "--tmpfs",
    "/tmp:rw,noexec,nosuid,size=64m",
    ...Object.keys(runtimeEnvironment)
      .sort()
      .flatMap((name) => ["--env", name]),
    image,
    "start",
    "--user-worker-request-idle-timeout",
    String(config.wallTimeMilliseconds + RUNTIME_SUPERVISOR_GRACE_MILLISECONDS),
    "--main-service",
    "/home/deno/functions/main",
  ];
  return {
    command: engine,
    args,
    environment: runtimeEnvironment,
    image,
    exposedEnvironmentNames,
    secretNames: Object.keys(secrets).sort(),
  };
}

export function formatLaunchPlan(plan: RuntimeLaunchPlan): string {
  return [plan.command, ...plan.args].map(shellQuote).join(" ");
}

export async function runLocalServe(config: LocalServeConfig): Promise<void> {
  const plan = createRuntimeLaunchPlan(config);
  process.stdout.write(
    `Mako function ${config.functionName} -> http://127.0.0.1:${config.port}/${config.projectId}/functions/v1/${config.functionName}\n`,
  );
  process.stdout.write(`runtime=${plan.image}; verifyJwt=${config.verifyJwt}\n`);
  if (config.dryRun) {
    process.stdout.write(`${formatLaunchPlan(plan)}\n`);
    return;
  }
  const child = spawn(plan.command, plan.args, {
    env: { ...process.env, ...plan.environment },
    stdio: ["inherit", "pipe", "pipe"],
  });
  const secretValues = plan.secretNames.map((name) => plan.environment[name] ?? "");
  pipeRedactedOutput(child.stdout, process.stdout, secretValues);
  pipeRedactedOutput(child.stderr, process.stderr, secretValues);
  const interrupt = () => child.kill("SIGINT");
  const terminate = () => child.kill("SIGTERM");
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  const exitCode = await new Promise<number>((resolveExit, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) => resolveExit(code ?? (signal === null ? 1 : 128)));
  });
  process.removeListener("SIGINT", interrupt);
  process.removeListener("SIGTERM", terminate);
  if (exitCode !== 0) {
    throw new Error(`edge runtime exited with status ${exitCode}`);
  }
}

/** Redacts exact active secret values from one complete runtime output string. */
export function redactRuntimeOutput(value: string, secretValues: readonly string[]): string {
  return [...new Set(secretValues)]
    .filter((secret) => secret.length > 0)
    .sort((left, right) => right.length - left.length)
    .reduce((redacted, secret) => redacted.replaceAll(secret, "[REDACTED]"), value);
}

function pipeRedactedOutput(
  input: Readable,
  output: Writable,
  secretValues: readonly string[],
): void {
  const secrets = [...new Set(secretValues)].filter((secret) => secret.length > 0);
  const decoder = new TextDecoder();
  let pending = "";
  const flushSafe = (final: boolean) => {
    pending = redactRuntimeOutput(pending, secrets);
    const retained = final ? 0 : longestPossibleSecretPrefix(pending, secrets);
    const safeLength = pending.length - retained;
    if (safeLength > 0) output.write(pending.slice(0, safeLength));
    pending = pending.slice(safeLength);
  };
  input.on("data", (chunk: Buffer) => {
    pending += decoder.decode(chunk, { stream: true });
    flushSafe(false);
  });
  input.on("end", () => {
    pending += decoder.decode();
    flushSafe(true);
  });
}

function longestPossibleSecretPrefix(value: string, secrets: readonly string[]): number {
  let longest = 0;
  for (const secret of secrets) {
    const maximum = Math.min(value.length, secret.length - 1);
    for (let length = maximum; length > longest; length -= 1) {
      if (value.endsWith(secret.slice(0, length))) {
        longest = length;
        break;
      }
    }
  }
  return longest;
}

function runtimePin(): RuntimePin {
  const path = fileURLToPath(new URL("../runtime/runtime-pin.json", import.meta.url));
  const pin = JSON.parse(readFileSync(path, "utf8")) as Partial<RuntimePin>;
  if (
    pin.provider !== "supabase-edge-runtime" ||
    pin.release !== "v1.74.3" ||
    pin.protocolVersion !== 1 ||
    typeof pin.imageRepository !== "string" ||
    !/^sha256:[a-f0-9]{64}$/u.test(pin.imageDigest ?? "")
  ) {
    throw new LocalServeConfigurationError("bundled edge runtime pin is invalid");
  }
  return pin as RuntimePin;
}

function readEnvironmentFiles(files: readonly string[]): Record<string, string> {
  const values: Record<string, string> = {};
  for (const file of files) {
    for (const [lineIndex, sourceLine] of readFileSync(file, "utf8").split(/\r?\n/u).entries()) {
      const line = sourceLine.trim();
      if (line === "" || line.startsWith("#")) continue;
      const separator = line.indexOf("=");
      const name = line.slice(0, separator).trim();
      let value = line.slice(separator + 1).trim();
      if (separator < 1 || !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/u.test(name)) {
        throw new LocalServeConfigurationError(`${file}:${lineIndex + 1} is not KEY=VALUE`);
      }
      if (name === "DENO_DIR" || name.startsWith("MAKO_") || name.startsWith("EDGE_RUNTIME_")) {
        throw new LocalServeConfigurationError(`${file}:${lineIndex + 1} uses a reserved name`);
      }
      if (
        value.length >= 2 &&
        ((value.startsWith('"') && value.endsWith('"')) ||
          (value.startsWith("'") && value.endsWith("'")))
      ) {
        value = value.slice(1, -1);
      }
      if (value.includes("\0")) {
        throw new LocalServeConfigurationError(`${file}:${lineIndex + 1} contains a NUL byte`);
      }
      if (Object.hasOwn(values, name)) {
        throw new LocalServeConfigurationError(`${file}:${lineIndex + 1} duplicates ${name}`);
      }
      values[name] = value;
    }
  }
  return values;
}

function readAndValidateJwks(path: string): string {
  let value: unknown;
  try {
    value = JSON.parse(readFileSync(path, "utf8"));
  } catch {
    throw new LocalServeConfigurationError("JWKS file must contain valid JSON");
  }
  if (!isRecord(value) || !Array.isArray(value.keys) || value.keys.length < 1) {
    throw new LocalServeConfigurationError("JWKS file must contain at least one Ed25519 key");
  }
  const keyIds = new Set<string>();
  for (const key of value.keys) {
    if (
      !isRecord(key) ||
      key.kty !== "OKP" ||
      key.crv !== "Ed25519" ||
      key.alg !== "EdDSA" ||
      typeof key.kid !== "string" ||
      !/^[A-Za-z0-9_-]{1,128}$/u.test(key.kid) ||
      typeof key.x !== "string" ||
      !/^[A-Za-z0-9_-]{43}$/u.test(key.x) ||
      (key.use !== undefined && key.use !== "sig") ||
      keyIds.has(key.kid)
    ) {
      throw new LocalServeConfigurationError("JWKS contains an invalid or duplicate Ed25519 key");
    }
    keyIds.add(key.kid);
  }
  return JSON.stringify(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function findContainerEngine(): "docker" | "podman" {
  for (const engine of ["docker", "podman"] as const) {
    if (commandOnPath(engine)) return engine;
  }
  throw new LocalServeConfigurationError("docker or podman is required for local function serve");
}

function commandOnPath(command: string): boolean {
  return (process.env.PATH ?? "").split(delimiter).some((directory) => {
    try {
      accessSync(join(directory, command), fsConstants.X_OK);
      return true;
    } catch {
      return false;
    }
  });
}

function one(options: ReadonlyMap<string, string[]>, name: string): string | undefined {
  const values = options.get(name);
  if (values !== undefined && values.length !== 1) {
    throw new LocalServeConfigurationError(`${name} may be specified only once`);
  }
  return values?.[0];
}

function required(options: ReadonlyMap<string, string[]>, name: string): string {
  const value = one(options, name);
  if (value === undefined) throw new LocalServeConfigurationError(`${name} is required`);
  return value;
}

function manyFiles(options: ReadonlyMap<string, string[]>, name: string, cwd: string): string[] {
  return (options.get(name) ?? []).map((value) => {
    const path = absolutePath(value, cwd);
    requireFile(path, name);
    return path;
  });
}

function optionalFile(
  options: ReadonlyMap<string, string[]>,
  name: string,
  cwd: string,
): string | undefined {
  const value = one(options, name);
  if (value === undefined) return undefined;
  const path = absolutePath(value, cwd);
  requireFile(path, name);
  return path;
}

function absolutePath(value: string, cwd: string): string {
  return isAbsolute(value) ? resolve(value) : resolve(cwd, value);
}

function requireDirectory(path: string, label: string): void {
  if (!statSync(path, { throwIfNoEntry: false })?.isDirectory()) {
    throw new LocalServeConfigurationError(`${label} does not exist: ${path}`);
  }
}

function requireFile(path: string, label: string): void {
  if (!statSync(path, { throwIfNoEntry: false })?.isFile()) {
    throw new LocalServeConfigurationError(`${label} does not exist: ${path}`);
  }
}

function validateRelativeEntrypoint(value: string): void {
  if (
    value === "" ||
    isAbsolute(value) ||
    value.split(/[\\/]/u).some((part) => part === "" || part === "." || part === "..")
  ) {
    throw new LocalServeConfigurationError("entrypoint must be a safe relative path");
  }
}

function validateIdentifier(value: string, pattern: RegExp, label: string): void {
  if (!pattern.test(value)) throw new LocalServeConfigurationError(`${label} is invalid`);
}

function validateApiUrl(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new LocalServeConfigurationError("API URL must be an absolute HTTP or HTTPS URL");
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new LocalServeConfigurationError("API URL must use HTTP or HTTPS");
  }
  url.pathname = url.pathname.replace(/\/+$/u, "");
  url.search = "";
  url.hash = "";
  return url.toString().replace(/\/$/u, "");
}

function rejectUnknownOptions(options: ReadonlyMap<string, string[]>): void {
  const known = new Set([
    "--api-url",
    "--container-engine",
    "--entrypoint",
    "--env-file",
    "--environment-id",
    "--function-name",
    "--jwks-file",
    "--jwt-audience",
    "--jwt-issuer",
    "--port",
    "--project-id",
    "--secret-file",
    "--wall-time-ms",
  ]);
  const unknown = [...options.keys()].find((option) => !known.has(option));
  if (unknown !== undefined) throw new LocalServeConfigurationError(`unknown option: ${unknown}`);
}

function shellQuote(value: string): string {
  return /^[A-Za-z0-9_./:@=-]+$/u.test(value) ? value : `'${value.replaceAll("'", "'\\''")}'`;
}
