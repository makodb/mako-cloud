// Mako's authenticated control surface for the pinned Supabase Edge Runtime.
// It intentionally imports no network modules and persists only AES-GCM
// ciphertext outside the container's tmpfs worker directory.

import { EDGE_SDK_SOURCE } from "./edge-sdk-source.ts";

type UserWorker = {
  fetch(request: Request, options?: { signal: AbortSignal }): Promise<Response>;
};

declare const EdgeRuntime: {
  applySupabaseTag(source: Request, destination: Request): void;
  readonly userWorkers: {
    create(options: {
      servicePath: string;
      maybeEntrypoint: string;
      memoryLimitMb: number;
      workerTimeoutMs: number;
      noModuleCache: boolean;
      envVars: string[][];
      forceCreate: boolean;
      cpuTimeSoftLimitMs: number;
      cpuTimeHardLimitMs: number;
      staticPatterns: string[];
      // Deno reads a missing grant as `null`, and an *empty list* as
      // "granted without restriction" -- a populated list is the restriction.
      // Every field a worker must not hold is therefore `null`, never `[]`.
      permissions: {
        allow_all: boolean;
        allow_env: string[] | null;
        allow_net: string[] | null;
        allow_read: string[] | null;
        allow_write: string[] | null;
        allow_import: string[] | null;
        allow_run: string[] | null;
        allow_ffi: string[] | null;
        allow_sys: string[] | null;
      };
      context: Record<string, unknown>;
    }): Promise<UserWorker>;
  };
};

declare const Deno: {
  readonly env: { get(name: string): string | undefined };
  mkdir(path: string, options: { recursive: boolean }): Promise<void>;
  readDir(path: string): AsyncIterable<{ name: string; isFile: boolean }>;
  readTextFile(path: string): Promise<string>;
  remove(path: string, options?: { recursive: boolean }): Promise<void>;
  stat(path: string): Promise<{ isFile: boolean }>;
  rename(oldPath: string, newPath: string): Promise<void>;
  writeFile(path: string, data: Uint8Array): Promise<void>;
  writeTextFile(path: string, data: string): Promise<void>;
};

const PROTOCOL_VERSION = 1;
const RUNTIME_RELEASE = "v1.74.3";
const SOURCE_COMMIT = "47d04fdd22e33ea3fd904576cf3248d963d903a9";
const VERSION_HEADER = "x-mako-runtime-protocol";
const REQUEST_ID_HEADER = "x-mako-request-id";
const AUTHORIZATION_HEADER = "x-mako-runtime-authorization";
const HEALTH_PATH = "/_mako/runtime/v1/health";
const LOAD_PATH = "/_mako/runtime/v1/deployments/load";
const PROBE_PATH = "/_mako/runtime/v1/deployments/probe";
const TEST_PATH = "/_mako/runtime/v1/deployments/test";
const LOGS_PATH = "/_mako/runtime/v1/deployments/logs";
const RETIRE_PATH = "/_mako/runtime/v1/deployments/retire";
const RETIRE_FUNCTION_PATH = "/_mako/runtime/v1/functions/retire";
const MAX_MANAGEMENT_BODY_BYTES = 32 * 1024 * 1024;
const MAX_BUNDLE_BYTES = 10 * 1024 * 1024;
const MAX_INVOCATION_BODY_BYTES = 10 * 1024 * 1024;
const MAX_LOGS_PER_DEPLOYMENT = 4_096;
// The response header a worker's console shim ships captured lines on. The
// supervisor consumes and strips it; it must never reach a caller.
const WORKER_LOG_HEADER = "x-mako-worker-logs";
const SHIM_MODULE = "__mako_console_shim.ts";
// The edge SDK is a first-party module of the runtime, not something a
// function vendors: its built source travels in this worker's module graph
// (`edge-sdk-source.ts`), the supervisor materializes it into each worker
// directory under a reserved name, and the worker's own import map -- which
// holds this one mapping and nothing else -- resolves the bare specifier onto
// it. A worker may read nothing outside its directory and has no network, so
// the module has to be inside it.
const SDK_SPECIFIER = "@mako-cloud/edge-sdk";
const SDK_MODULE = "__mako_edge_sdk.mjs";
const MAX_SHIPPED_LOG_LINES = 64;
const MAX_SHIPPED_LOG_BYTES = 16 * 1024;
// Total message text retained per deployment. JSON escaping can inflate a
// character to six bytes, so this keeps the encrypted state far below the
// 32 MiB restore ceiling even in the worst case.
const MAX_RETAINED_LOG_CHARS = 1024 * 1024;
const MAX_LOG_PAGE = 1_000;
const MAX_RESPONSE_HEADERS = 128;

type Tenant = { projectId: string; environmentId: string };
type DeploymentAddress = Tenant & { functionName: string; version: number };
type ProtocolDeploymentAddress = { tenant: Tenant; functionName: string; version: number };
type FunctionAddress = { tenant: Tenant; functionName: string };
type OutboundNetwork = { mode: "deny_all" } | { mode: "allow_list"; hosts: string[] };
type RuntimeLimits = {
  cpuMilliseconds: number;
  wallMilliseconds: number;
  memoryBytes: number;
  requestBytes: number;
  responseBytes: number;
  concurrency: number;
  outboundNetwork: OutboundNetwork;
};
type SecretReference = { name: string; version: number };
type SensitiveSecret = { reference: SecretReference; value: string };
type DeploymentManifest = {
  protocolVersion: number;
  deployment: ProtocolDeploymentAddress;
  bundleDigest: string;
  bundleFormat: "source_archive_v1" | "prebuilt";
  entrypoint: string;
  runtimeRelease: string;
  limits: RuntimeLimits;
  verifyJwt: boolean;
  secretVersions: SecretReference[];
};
type SensitiveLoad = {
  protocolVersion: number;
  requestId: string;
  manifest: DeploymentManifest;
  bundleBase64: string;
  secrets: SensitiveSecret[];
};
type DeploymentLog = {
  deployment: ProtocolDeploymentAddress;
  timestampUnixMilliseconds: number;
  level: "debug" | "info" | "warn" | "error";
  message: string;
  correlationId: string;
  region: string;
};
type PersistedDeployment = {
  formatVersion: 1;
  requestDigest: string;
  load: SensitiveLoad;
  logs: DeploymentLog[];
};
type EncryptedState = {
  formatVersion: 1;
  ivBase64: string;
  ciphertextBase64: string;
};
type DeploymentRecord = PersistedDeployment & {
  worker: UserWorker;
  activeInvocations: number;
};
type SourceArchive = {
  formatVersion: number;
  entrypoint: string;
  modules: Record<string, string>;
  resolvedImports: Record<string, Record<string, string>>;
};

export class RuntimeSupervisor {
  readonly #authorization: string;
  readonly #region: string;
  readonly #statePath: string;
  readonly #workerPath: string;
  readonly #key: CryptoKey;
  readonly #deployments = new Map<string, DeploymentRecord>();

  private constructor(
    authorization: string,
    region: string,
    statePath: string,
    workerPath: string,
    key: CryptoKey,
  ) {
    this.#authorization = authorization;
    this.#region = region;
    this.#statePath = statePath;
    this.#workerPath = workerPath;
    this.#key = key;
  }

  static async open(): Promise<RuntimeSupervisor> {
    const authorization = requiredEnvironment("MAKO_RUNTIME_AUTHORIZATION");
    if (authorization.length < 32 || authorization.length > 1_024 || hasControl(authorization)) {
      throw new Error("invalid runtime authorization");
    }
    const region = requiredEnvironment("MAKO_RUNTIME_REGION");
    if (!validRegion(region)) throw new Error("invalid runtime region");
    const statePath = requiredEnvironment("MAKO_RUNTIME_STATE_PATH");
    const workerPath = requiredEnvironment("MAKO_RUNTIME_WORKER_PATH");
    if (
      !statePath.startsWith("/") ||
      (!workerPath.startsWith("/tmp/") && workerPath !== "/var/lib/mako-runtime-workers")
    ) {
      throw new Error("invalid runtime storage paths");
    }
    const stateKey = decodeHex(requiredEnvironment("MAKO_RUNTIME_STATE_KEY"));
    if (stateKey.length !== 32) throw new Error("invalid runtime state key");
    const key = await crypto.subtle.importKey("raw", bytesBuffer(stateKey), "AES-GCM", false, [
      "encrypt",
      "decrypt",
    ]);
    const supervisor = new RuntimeSupervisor(authorization, region, statePath, workerPath, key);
    await supervisor.#restore();
    return supervisor;
  }

  async manage(request: Request): Promise<Response> {
    const requestId = requestIdentifier(request.headers.get(REQUEST_ID_HEADER));
    if (
      !(await constantTimeEqual(request.headers.get(AUTHORIZATION_HEADER), this.#authorization))
    ) {
      return runtimeError(401, "runtime_unavailable", requestId, false);
    }
    if (request.headers.get(VERSION_HEADER) !== String(PROTOCOL_VERSION)) {
      return runtimeError(426, "protocol_mismatch", requestId, false);
    }
    const path = new URL(request.url).pathname;
    if (path === HEALTH_PATH) {
      if (request.method !== "GET")
        return runtimeError(405, "invalid_deployment", requestId, false);
      return runtimeJson(
        200,
        {
          protocolVersion: PROTOCOL_VERSION,
          runtimeRelease: RUNTIME_RELEASE,
          sourceCommit: SOURCE_COMMIT,
          region: this.#region,
          ready: true,
        },
        requestId,
      );
    }
    if (request.method !== "POST") {
      return runtimeError(405, "invalid_deployment", requestId, false);
    }

    let body: Uint8Array;
    try {
      body = await boundedBody(request, MAX_MANAGEMENT_BODY_BYTES);
    } catch {
      return runtimeError(413, "request_too_large", requestId, false);
    }
    let payload: unknown;
    try {
      payload = JSON.parse(new TextDecoder().decode(body));
    } catch {
      return runtimeError(400, "invalid_deployment", requestId, false);
    }

    try {
      switch (path) {
        case LOAD_PATH:
          return await this.#load(payload, body, requestId);
        case PROBE_PATH:
          return this.#probe(payload, requestId);
        case TEST_PATH:
          return await this.#test(payload, requestId, request);
        case LOGS_PATH:
          return this.#logs(payload, requestId);
        case RETIRE_PATH:
          return await this.#retire(payload, requestId);
        case RETIRE_FUNCTION_PATH:
          return await this.#retireFunction(payload, requestId);
        default:
          return runtimeError(404, "deployment_not_found", requestId, false);
      }
    } catch {
      return runtimeError(503, "runtime_unavailable", requestId, true);
    }
  }

  async tryInvoke(request: Request): Promise<Response | null> {
    const url = new URL(request.url);
    const match =
      /^\/(prj_[A-Za-z0-9_-]{8,64})\/functions\/v1\/([a-z][a-z0-9-]{0,62})(\/.*)?$/u.exec(
        url.pathname,
      );
    if (match?.[1] === undefined || match[2] === undefined) return null;
    const environmentId = request.headers.get("x-mako-runtime-environment-id");
    const versionSource = request.headers.get("x-mako-runtime-deployment-version");
    if (!validEnvironmentId(environmentId) || !/^[1-9][0-9]{0,19}$/u.test(versionSource ?? "")) {
      return null;
    }
    const version = Number(versionSource);
    if (!Number.isSafeInteger(version)) return null;
    const address: DeploymentAddress = {
      projectId: match[1],
      environmentId,
      functionName: match[2],
      version,
    };
    const record = this.#deployments.get(deploymentKey(address));
    if (record === undefined) {
      return publicFailure(
        404,
        "not_found",
        "function deployment was not found",
        requestIdentifier(request.headers.get("x-request-id")),
      );
    }
    const requestId = requestIdentifier(request.headers.get("x-request-id"));
    const functionPath = match[3] ?? "/";
    return await this.#invoke(record, request, functionPath + url.search, requestId, true);
  }

  async #restore(): Promise<void> {
    await Deno.mkdir(this.#statePath, { recursive: true });
    await Deno.remove(this.#workerPath, { recursive: true }).catch(() => undefined);
    await Deno.mkdir(this.#workerPath, { recursive: true });
    for await (const entry of Deno.readDir(this.#statePath)) {
      if (!entry.isFile || !entry.name.endsWith(".state")) continue;
      // One deployment's unrestorable state must not keep every other
      // tenant's functions down: quarantine it and keep booting. The
      // affected function redeploys; the others never notice. This covers
      // the whole restore of the entry -- decrypt, validation, and worker
      // materialization -- because a boot-time worker error is just as
      // fatal to the loop as an unreadable file.
      try {
        const source = await Deno.readTextFile(`${this.#statePath}/${entry.name}`);
        const persisted = await this.#decrypt(source);
        validatePersisted(persisted);
        const address = flattenAddress(persisted.load.manifest.deployment);
        const expected = `${await sha256Hex(deploymentKey(address))}.state`;
        if (entry.name !== expected) throw new Error("runtime state address mismatch");
        const worker = await materializeWorker(persisted.load, this.#workerPath, this.#region);
        this.#deployments.set(deploymentKey(address), {
          ...persisted,
          worker,
          activeInvocations: 0,
        });
      } catch {
        await Deno.rename(
          `${this.#statePath}/${entry.name}`,
          `${this.#statePath}/${entry.name}.quarantined`,
        ).catch(() => undefined);
      }
    }
  }

  async #load(payload: unknown, body: Uint8Array, requestId: string): Promise<Response> {
    if (!isSensitiveLoad(payload) || payload.requestId !== requestId) {
      return runtimeError(400, "invalid_deployment", requestId, false);
    }
    const address = flattenAddress(payload.manifest.deployment);
    const key = deploymentKey(address);
    const digest = await requestDigest(payload);
    const existing = this.#deployments.get(key);
    if (existing !== undefined) {
      if (existing.requestDigest !== digest) {
        return runtimeError(409, "invalid_deployment", requestId, false);
      }
      return deploymentStatus(payload.manifest.deployment, "healthy", this.#region, requestId);
    }
    if (body.length > MAX_MANAGEMENT_BODY_BYTES) {
      return runtimeError(413, "request_too_large", requestId, false);
    }
    const bundle = decodeBase64(payload.bundleBase64, MAX_BUNDLE_BYTES);
    if ((await sha256Digest(bundle)) !== payload.manifest.bundleDigest) {
      return runtimeError(400, "invalid_deployment", requestId, false);
    }
    const persisted: PersistedDeployment = {
      formatVersion: 1,
      requestDigest: digest,
      load: payload,
      logs: [],
    };
    const worker = await materializeWorker(payload, this.#workerPath, this.#region);
    const record: DeploymentRecord = { ...persisted, worker, activeInvocations: 0 };
    appendLog(record, "info", "deployment_loaded", requestId, this.#region);
    await this.#persist(record);
    this.#deployments.set(key, record);
    return deploymentStatus(payload.manifest.deployment, "healthy", this.#region, requestId);
  }

  #probe(payload: unknown, requestId: string): Response {
    const deployment = operationDeployment(payload, requestId);
    if (deployment === null) return runtimeError(400, "invalid_deployment", requestId, false);
    const record = this.#deployments.get(deploymentKey(flattenAddress(deployment)));
    if (record === undefined) return runtimeError(404, "deployment_not_found", requestId, false);
    return deploymentStatus(deployment, "healthy", this.#region, requestId);
  }

  async #test(payload: unknown, requestId: string, incoming: Request): Promise<Response> {
    if (!isTestRequest(payload) || payload.requestId !== requestId) {
      return runtimeError(400, "invalid_deployment", requestId, false);
    }
    const record = this.#deployments.get(deploymentKey(flattenAddress(payload.deployment)));
    if (record === undefined) return runtimeError(404, "deployment_not_found", requestId, false);
    const body = decodeBase64(
      payload.bodyBase64,
      Math.min(MAX_INVOCATION_BODY_BYTES, record.load.manifest.limits.requestBytes),
    );
    const headers = safeRequestHeaders(payload.headers);
    const target = new Request(`http://runtime.local${payload.path}`, {
      method: payload.method,
      headers,
      body: payload.method === "GET" || payload.method === "HEAD" ? null : bytesBuffer(body),
    });
    // The test request is built here rather than received, and the runtime
    // refuses to hand a user worker a request that carries no tag from one it
    // received: every console test answered worker_crashed. The tag comes from
    // the supervisor request that asked for the test.
    const response = await this.#invoke(record, target, payload.path, requestId, false, incoming);
    const responseBody = new Uint8Array(await response.arrayBuffer());
    return runtimeJson(
      200,
      {
        protocolVersion: PROTOCOL_VERSION,
        requestId,
        deployment: payload.deployment,
        status: response.status,
        headers: safeResponseHeaders(response.headers),
        bodyBase64: encodeBase64(responseBody),
      },
      requestId,
    );
  }

  #logs(payload: unknown, requestId: string): Response {
    if (!isLogQuery(payload) || payload.requestId !== requestId) {
      return runtimeError(400, "invalid_deployment", requestId, false);
    }
    const all = [...this.#deployments.values()]
      .filter((record) => sameFunction(record.load.manifest.deployment, payload.function))
      .flatMap((record) => record.logs)
      .sort((left, right) => left.timestampUnixMilliseconds - right.timestampUnixMilliseconds);
    const offset = decodeCursor(payload.cursor, all.length);
    if (offset === null) return runtimeError(400, "invalid_deployment", requestId, false);
    const items = all.slice(offset, offset + payload.limit);
    const next = offset + items.length < all.length ? `runtime1.${offset + items.length}` : null;
    return runtimeJson(
      200,
      {
        protocolVersion: PROTOCOL_VERSION,
        requestId,
        function: payload.function,
        items,
        nextCursor: next,
      },
      requestId,
    );
  }

  async #retire(payload: unknown, requestId: string): Promise<Response> {
    const deployment = operationDeployment(payload, requestId);
    if (deployment === null) return runtimeError(400, "invalid_deployment", requestId, false);
    const address = flattenAddress(deployment);
    const key = deploymentKey(address);
    if (!this.#deployments.has(key))
      return runtimeError(404, "deployment_not_found", requestId, false);
    await this.#remove(address);
    return deploymentStatus(deployment, "retired", this.#region, requestId);
  }

  async #retireFunction(payload: unknown, requestId: string): Promise<Response> {
    const functionAddress = operationFunction(payload, requestId);
    if (functionAddress === null) return runtimeError(400, "invalid_deployment", requestId, false);
    const addresses = [...this.#deployments.values()]
      .map((record) => flattenAddress(record.load.manifest.deployment))
      .filter((address) => sameFlatFunction(address, functionAddress));
    for (const address of addresses) await this.#remove(address);
    return runtimeJson(
      200,
      {
        protocolVersion: PROTOCOL_VERSION,
        requestId,
        function: functionAddress,
        retiredVersions: addresses.length,
      },
      requestId,
    );
  }

  async #invoke(
    record: DeploymentRecord,
    source: Request,
    pathAndQuery: string,
    requestId: string,
    tagRequest: boolean,
    tagSource: Request | null = tagRequest ? source : null,
  ): Promise<Response> {
    const limits = record.load.manifest.limits;
    if (record.activeInvocations >= limits.concurrency) {
      return tagRequest
        ? publicFailure(429, "internal", "function concurrency limit was reached", requestId)
        : runtimeError(429, "concurrency_limited", requestId, true);
    }
    let body: Uint8Array;
    try {
      body =
        source.method === "GET" || source.method === "HEAD"
          ? new Uint8Array()
          : await boundedBody(source, Math.min(MAX_INVOCATION_BODY_BYTES, limits.requestBytes));
    } catch {
      return tagRequest
        ? publicFailure(413, "invalid_request", "function request is too large", requestId)
        : runtimeError(413, "request_too_large", requestId, false);
    }
    const headers = new Headers(source.headers);
    stripRuntimeHeaders(headers);
    headers.set("x-mako-request-id", requestId);
    const target = new Request(`http://runtime.local${pathAndQuery}`, {
      method: source.method,
      headers,
      body: source.method === "GET" || source.method === "HEAD" ? null : bytesBuffer(body),
    });
    if (tagSource !== null) EdgeRuntime.applySupabaseTag(tagSource, target);
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), limits.wallMilliseconds);
    record.activeInvocations += 1;
    try {
      const address = flattenAddress(record.load.manifest.deployment);
      const worker = await createWorker(
        record.load,
        workerDirectory(this.#workerPath, address),
        this.#region,
        false,
      );
      record.worker = worker;
      const response = await worker.fetch(target, { signal: controller.signal });
      const responseBody = new Uint8Array(await response.arrayBuffer());
      if (responseBody.length > Math.min(MAX_INVOCATION_BODY_BYTES, limits.responseBytes)) {
        throw new Error("response limit exceeded");
      }
      for (const line of shippedWorkerLogs(response.headers)) {
        appendLog(record, line.level, line.message, requestId, this.#region);
      }
      appendLog(record, "info", "invocation_completed", requestId, this.#region);
      // A failing response is worth a retained line of its own: the lifecycle
      // echo above is not kept, so a function that answered 500 on every call
      // used to leave nothing in its logs to say so.
      if (response.status >= 500) {
        appendLog(
          record,
          "error",
          `function responded with status ${response.status}`,
          requestId,
          this.#region,
        );
      }
      await this.#persist(record);
      const responseHeaders = new Headers(response.headers);
      responseHeaders.delete(WORKER_LOG_HEADER);
      responseHeaders.set("x-mako-request-id", requestId);
      return new Response(source.method === "HEAD" ? null : responseBody, {
        status: response.status,
        statusText: response.statusText,
        headers: responseHeaders,
      });
    } catch {
      appendLog(record, "error", "invocation_failed", requestId, this.#region);
      await this.#persist(record).catch(() => undefined);
      return tagRequest
        ? publicFailure(500, "internal", "function invocation failed", requestId)
        : runtimeError(503, "worker_crashed", requestId, true);
    } finally {
      clearTimeout(timeout);
      record.activeInvocations -= 1;
    }
  }

  async #persist(record: DeploymentRecord): Promise<void> {
    const address = flattenAddress(record.load.manifest.deployment);
    const filename = `${await sha256Hex(deploymentKey(address))}.state`;
    const encrypted = await this.#encrypt({
      formatVersion: 1,
      requestDigest: record.requestDigest,
      load: record.load,
      logs: record.logs.slice(-MAX_LOGS_PER_DEPLOYMENT),
    });
    const temporary = `${this.#statePath}/.${filename}.${crypto.randomUUID()}.tmp`;
    await Deno.writeTextFile(temporary, encrypted);
    await Deno.rename(temporary, `${this.#statePath}/${filename}`);
  }

  async #remove(address: DeploymentAddress): Promise<void> {
    this.#deployments.delete(deploymentKey(address));
    const filename = `${await sha256Hex(deploymentKey(address))}.state`;
    await Deno.remove(`${this.#statePath}/${filename}`).catch(() => undefined);
    await Deno.remove(workerDirectory(this.#workerPath, address), { recursive: true }).catch(
      () => undefined,
    );
  }

  async #encrypt(value: PersistedDeployment): Promise<string> {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const plaintext = new TextEncoder().encode(JSON.stringify(value));
    const ciphertext = new Uint8Array(
      await crypto.subtle.encrypt({ name: "AES-GCM", iv }, this.#key, plaintext),
    );
    const envelope: EncryptedState = {
      formatVersion: 1,
      ivBase64: encodeBase64(iv),
      ciphertextBase64: encodeBase64(ciphertext),
    };
    return JSON.stringify(envelope);
  }

  async #decrypt(source: string): Promise<PersistedDeployment> {
    const envelope: unknown = JSON.parse(source);
    if (!isEncryptedState(envelope)) throw new Error("invalid runtime state envelope");
    const iv = decodeBase64(envelope.ivBase64, 12);
    if (iv.length !== 12) throw new Error("invalid runtime state nonce");
    const ciphertext = decodeBase64(envelope.ciphertextBase64, MAX_MANAGEMENT_BODY_BYTES + 64);
    const plaintext = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: bytesBuffer(iv) },
      this.#key,
      bytesBuffer(ciphertext),
    );
    return JSON.parse(new TextDecoder().decode(plaintext)) as PersistedDeployment;
  }
}

async function materializeWorker(
  load: SensitiveLoad,
  root: string,
  region: string,
): Promise<UserWorker> {
  const address = flattenAddress(load.manifest.deployment);
  const directory = workerDirectory(root, address);
  await Deno.remove(directory, { recursive: true }).catch(() => undefined);
  await Deno.mkdir(directory, { recursive: true });
  const bundle = decodeBase64(load.bundleBase64, MAX_BUNDLE_BYTES);
  if (load.manifest.bundleFormat === "source_archive_v1") {
    const archive: unknown = JSON.parse(new TextDecoder().decode(bundle));
    if (!isSourceArchive(archive) || archive.entrypoint !== load.manifest.entrypoint) {
      throw new Error("invalid source archive");
    }
    let decodedBytes = 0;
    const entrypointSources = new Map<string, string>();
    for (const [path, encoded] of Object.entries(archive.modules)) {
      if (!validRelativePath(path)) throw new Error("invalid source module path");
      const contents = decodeBase64(encoded, MAX_BUNDLE_BYTES);
      decodedBytes += contents.length;
      if (decodedBytes > MAX_BUNDLE_BYTES) throw new Error("source archive exceeds limit");
      const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
      if (parent !== "") await Deno.mkdir(`${directory}/${parent}`, { recursive: true });
      if (path.endsWith(".wasm")) {
        await Deno.writeFile(`${directory}/${path}`, contents);
      } else {
        const rewritten = rewriteImports(
          new TextDecoder("utf-8", { fatal: true }).decode(contents),
          path,
          archive.resolvedImports[path] ?? {},
        );
        if (path === load.manifest.entrypoint) entrypointSources.set(path, rewritten);
        await Deno.writeTextFile(`${directory}/${path}`, rewritten);
      }
    }
    // Console output never leaves a worker isolate on its own: the shim
    // captures it during a request and ships it back on a response header
    // the supervisor strips and retains. An entry module that exports default
    // is wrapped by a shim that exports default in turn; a Deno.serve-style
    // module registers its handler as an import side effect, so its shim
    // exports nothing, wraps Deno.serve, and imports the module after (a shim
    // that exported default around it would serve nothing, a boot error). A
    // bundle that carries a module by the shim's name goes uncaptured.
    const entrySource = entrypointSources.get(load.manifest.entrypoint);
    if (archive.modules[SHIM_MODULE] === undefined && entrySource !== undefined) {
      if (/(^|\n)\s*export\s+default\b/u.test(entrySource)) {
        await Deno.writeTextFile(
          `${directory}/${SHIM_MODULE}`,
          consoleShimSource(load.manifest.entrypoint),
        );
      } else if (/\bDeno\.serve\s*\(/u.test(entrySource)) {
        // The style the user book teaches: the module registers its handler
        // with Deno.serve as it loads. The shim wraps Deno.serve before the
        // module is imported, so the handler it registers ships its lines too.
        await Deno.writeTextFile(
          `${directory}/${SHIM_MODULE}`,
          serveShimSource(load.manifest.entrypoint),
        );
      }
    }
  } else {
    const entrypoint = load.manifest.entrypoint;
    const parent = entrypoint.includes("/") ? entrypoint.slice(0, entrypoint.lastIndexOf("/")) : "";
    if (parent !== "") await Deno.mkdir(`${directory}/${parent}`, { recursive: true });
    await Deno.writeFile(`${directory}/${entrypoint}`, bundle);
  }
  // Written last: the platform's module always wins over a bundle that
  // carries the reserved name. Uploads that claim it are refused at
  // validation, so this only decides restores of state written before that
  // rule existed.
  await Deno.writeTextFile(`${directory}/${SDK_MODULE}`, EDGE_SDK_SOURCE);
  return await createWorker(load, directory, region, true);
}

async function createWorker(
  load: SensitiveLoad,
  directory: string,
  region: string,
  forceCreate: boolean,
): Promise<UserWorker> {
  const address = flattenAddress(load.manifest.deployment);
  const environment = load.secrets.map((secret) => [secret.reference.name, secret.value]);
  environment.push(
    ["MAKO_API_URL", requiredEnvironment("MAKO_API_URL")],
    ["MAKO_PROJECT_ID", address.projectId],
    ["MAKO_ENVIRONMENT_ID", address.environmentId],
    ["MAKO_FUNCTION_NAME", address.functionName],
    ["MAKO_FUNCTION_VERSION", String(address.version)],
    ["MAKO_RUNTIME_REGION", region],
  );
  const limits = load.manifest.limits;
  const entrypoint = (await fileExists(`${directory}/${SHIM_MODULE}`))
    ? SHIM_MODULE
    : load.manifest.entrypoint;
  return await EdgeRuntime.userWorkers.create({
    servicePath: directory,
    maybeEntrypoint: `file://${directory}/${entrypoint}`,
    memoryLimitMb: Math.max(1, Math.ceil(limits.memoryBytes / (1024 * 1024))),
    workerTimeoutMs: limits.wallMilliseconds,
    noModuleCache: false,
    envVars: environment,
    forceCreate,
    cpuTimeSoftLimitMs: limits.cpuMilliseconds,
    cpuTimeHardLimitMs: limits.cpuMilliseconds,
    staticPatterns: [`${directory}/**/*.wasm`],
    // Deny by default. In Deno's permission model an empty list is a grant
    // without restriction and `null` is no grant at all, so everything a
    // function has no business holding is `null`: it may not write, spawn a
    // process, open a native library, read the host's identity, or import a
    // module from anywhere but its own directory. What remains is bounded:
    // the environment names the platform gave it, its own worker directory,
    // and the origins its egress policy allows.
    permissions: {
      allow_all: false,
      allow_env: environment.map(([name]) => name ?? "").filter((name) => name !== ""),
      allow_net: networkGrants(limits.outboundNetwork),
      allow_read: [directory],
      allow_write: null,
      allow_import: null,
      allow_run: null,
      allow_ffi: null,
      allow_sys: null,
    },
    context: {
      // The runtime reads `importMapPath` out of the worker context. It holds
      // exactly one first-party mapping: a function cannot introduce its own,
      // and no specifier resolves outside this directory.
      importMapPath: firstPartyImportMap(directory),
      runtimeProtocol: PROTOCOL_VERSION,
      projectId: address.projectId,
      environmentId: address.environmentId,
      functionName: address.functionName,
      version: address.version,
      region,
      mode: "production",
    },
  });
}

function isSensitiveLoad(value: unknown): value is SensitiveLoad {
  if (
    !isExactRecord(value, ["protocolVersion", "requestId", "manifest", "bundleBase64", "secrets"])
  )
    return false;
  if (
    value.protocolVersion !== PROTOCOL_VERSION ||
    !validRequestId(value.requestId) ||
    typeof value.bundleBase64 !== "string" ||
    !Array.isArray(value.secrets) ||
    value.secrets.length > 64
  )
    return false;
  if (!isManifest(value.manifest)) return false;
  if (!value.secrets.every(isSensitiveSecret)) return false;
  const manifestReferences = value.manifest.secretVersions.map(referenceKey).sort();
  const suppliedReferences = value.secrets.map((secret) => referenceKey(secret.reference)).sort();
  return JSON.stringify(manifestReferences) === JSON.stringify(suppliedReferences);
}

function isManifest(value: unknown): value is DeploymentManifest {
  if (
    !isExactRecord(value, [
      "protocolVersion",
      "deployment",
      "bundleDigest",
      "bundleFormat",
      "entrypoint",
      "runtimeRelease",
      "limits",
      "verifyJwt",
      "secretVersions",
    ])
  )
    return false;
  return (
    value.protocolVersion === PROTOCOL_VERSION &&
    validProtocolAddress(value.deployment) &&
    typeof value.bundleDigest === "string" &&
    /^sha256:[0-9a-f]{64}$/u.test(value.bundleDigest) &&
    (value.bundleFormat === "source_archive_v1" || value.bundleFormat === "prebuilt") &&
    typeof value.entrypoint === "string" &&
    validRelativePath(value.entrypoint) &&
    value.runtimeRelease === RUNTIME_RELEASE &&
    typeof value.verifyJwt === "boolean" &&
    isLimits(value.limits) &&
    Array.isArray(value.secretVersions) &&
    value.secretVersions.length <= 64 &&
    value.secretVersions.every(isSecretReference) &&
    new Set(value.secretVersions.map(referenceKey)).size === value.secretVersions.length
  );
}

function isLimits(value: unknown): value is RuntimeLimits {
  if (
    !isExactRecord(value, [
      "cpuMilliseconds",
      "wallMilliseconds",
      "memoryBytes",
      "requestBytes",
      "responseBytes",
      "concurrency",
      "outboundNetwork",
    ])
  )
    return false;
  return (
    boundedInteger(value.cpuMilliseconds, 1, 60_000) &&
    boundedInteger(value.wallMilliseconds, 100, 300_000) &&
    boundedInteger(value.memoryBytes, 1024 * 1024, 1024 * 1024 * 1024) &&
    boundedInteger(value.requestBytes, 1, MAX_INVOCATION_BODY_BYTES) &&
    boundedInteger(value.responseBytes, 1, MAX_INVOCATION_BODY_BYTES) &&
    boundedInteger(value.concurrency, 1, 256) &&
    isOutboundNetwork(value.outboundNetwork)
  );
}

/**
 * The manifest's egress policy: `deny_all`, or an allowlist carrying hosts and
 * nothing else. The host rules mirror the control plane's protocol validation
 * -- DNS names only, never an IP literal or a link-local metadata name -- so a
 * manifest this process was handed is checked here too, not trusted for having
 * been checked once elsewhere.
 */
function isOutboundNetwork(value: unknown): value is OutboundNetwork {
  if (isExactRecord(value, ["mode"]) && value.mode === "deny_all") return true;
  return (
    isExactRecord(value, ["mode", "hosts"]) &&
    value.mode === "allow_list" &&
    Array.isArray(value.hosts) &&
    value.hosts.length >= 1 &&
    value.hosts.length <= 64 &&
    value.hosts.every(validOutboundHost) &&
    new Set(value.hosts).size === value.hosts.length
  );
}

function validOutboundHost(value: unknown): boolean {
  if (typeof value !== "string" || value === "" || value.length > 253) return false;
  const labels = value.split(".");
  const labelsAreValid = labels.every(
    (label) =>
      label !== "" &&
      label.length <= 63 &&
      !label.startsWith("-") &&
      !label.endsWith("-") &&
      /^[a-z0-9-]+$/.test(label),
  );
  const isIpLike = /^\d+(\.\d+){3}$/.test(value) || value.includes(":");
  const denied = [
    "localhost",
    "metadata",
    "metadata.google.internal",
    "instance-data.ec2.internal",
  ];
  return labelsAreValid && !isIpLike && !denied.includes(value);
}

function isSensitiveSecret(value: unknown): value is SensitiveSecret {
  return (
    isExactRecord(value, ["reference", "value"]) &&
    isSecretReference(value.reference) &&
    typeof value.value === "string" &&
    value.value.length > 0 &&
    value.value.length <= 64 * 1024 &&
    !value.value.includes("\0")
  );
}

function isSecretReference(value: unknown): value is SecretReference {
  return (
    isExactRecord(value, ["name", "version"]) &&
    typeof value.name === "string" &&
    /^[A-Z_][A-Z0-9_]{0,127}$/u.test(value.name) &&
    boundedInteger(value.version, 1, Number.MAX_SAFE_INTEGER)
  );
}

function operationDeployment(value: unknown, requestId: string): ProtocolDeploymentAddress | null {
  return isExactRecord(value, ["protocolVersion", "requestId", "deployment"]) &&
    value.protocolVersion === PROTOCOL_VERSION &&
    value.requestId === requestId &&
    validProtocolAddress(value.deployment)
    ? value.deployment
    : null;
}

function operationFunction(value: unknown, requestId: string): FunctionAddress | null {
  return isExactRecord(value, ["protocolVersion", "requestId", "function"]) &&
    value.protocolVersion === PROTOCOL_VERSION &&
    value.requestId === requestId &&
    validFunctionAddress(value.function)
    ? value.function
    : null;
}

function isTestRequest(value: unknown): value is {
  protocolVersion: number;
  requestId: string;
  deployment: ProtocolDeploymentAddress;
  method: string;
  path: string;
  headers: [string, string][];
  bodyBase64: string;
} {
  return (
    isExactRecord(value, [
      "protocolVersion",
      "requestId",
      "deployment",
      "method",
      "path",
      "headers",
      "bodyBase64",
    ]) &&
    value.protocolVersion === PROTOCOL_VERSION &&
    validRequestId(value.requestId) &&
    validProtocolAddress(value.deployment) &&
    typeof value.method === "string" &&
    ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].includes(value.method) &&
    typeof value.path === "string" &&
    value.path.startsWith("/") &&
    value.path.length <= 8_192 &&
    !value.path.includes("\\") &&
    !hasControl(value.path) &&
    validHeaderPairs(value.headers) &&
    typeof value.bodyBase64 === "string"
  );
}

function isLogQuery(value: unknown): value is {
  protocolVersion: number;
  requestId: string;
  function: FunctionAddress;
  cursor: string | null;
  limit: number;
} {
  return (
    isExactRecord(value, ["protocolVersion", "requestId", "function", "cursor", "limit"]) &&
    value.protocolVersion === PROTOCOL_VERSION &&
    validRequestId(value.requestId) &&
    validFunctionAddress(value.function) &&
    (value.cursor === null ||
      (typeof value.cursor === "string" &&
        value.cursor.length <= 2_048 &&
        !hasControl(value.cursor))) &&
    boundedInteger(value.limit, 1, MAX_LOG_PAGE)
  );
}

function validatePersisted(value: PersistedDeployment): void {
  if (
    !isExactRecord(value, ["formatVersion", "requestDigest", "load", "logs"]) ||
    value.formatVersion !== 1 ||
    typeof value.requestDigest !== "string" ||
    !/^[0-9a-f]{64}$/u.test(value.requestDigest) ||
    !isSensitiveLoad(value.load) ||
    !Array.isArray(value.logs) ||
    value.logs.length > MAX_LOGS_PER_DEPLOYMENT ||
    !value.logs.every(isDeploymentLog)
  ) {
    throw new Error("invalid persisted runtime state");
  }
}

function isDeploymentLog(value: unknown): value is DeploymentLog {
  return (
    isExactRecord(value, [
      "deployment",
      "timestampUnixMilliseconds",
      "level",
      "message",
      "correlationId",
      "region",
    ]) &&
    validProtocolAddress(value.deployment) &&
    boundedInteger(value.timestampUnixMilliseconds, 1, Number.MAX_SAFE_INTEGER) &&
    ["debug", "info", "warn", "error"].includes(String(value.level)) &&
    typeof value.message === "string" &&
    value.message.length > 0 &&
    value.message.length <= 16 * 1024 &&
    typeof value.correlationId === "string" &&
    /^[A-Za-z0-9_-]{8,132}$/u.test(value.correlationId) &&
    typeof value.region === "string" &&
    validRegion(value.region)
  );
}

function isSourceArchive(value: unknown): value is SourceArchive {
  return (
    isExactRecord(value, ["formatVersion", "entrypoint", "modules", "resolvedImports"]) &&
    value.formatVersion === 1 &&
    typeof value.entrypoint === "string" &&
    validRelativePath(value.entrypoint) &&
    isRecord(value.modules) &&
    Object.keys(value.modules).length > 0 &&
    Object.keys(value.modules).length <= 512 &&
    Object.entries(value.modules).every(
      ([path, encoded]) => validRelativePath(path) && typeof encoded === "string",
    ) &&
    isRecord(value.resolvedImports) &&
    Object.entries(value.resolvedImports).every(
      ([path, imports]) =>
        validRelativePath(path) &&
        isRecord(imports) &&
        Object.entries(imports).every(
          ([specifier, target]) =>
            specifier.length <= 512 &&
            typeof target === "string" &&
            (target === SDK_SPECIFIER || validRelativePath(target)),
        ),
    )
  );
}

function isEncryptedState(value: unknown): value is EncryptedState {
  return (
    isExactRecord(value, ["formatVersion", "ivBase64", "ciphertextBase64"]) &&
    value.formatVersion === 1 &&
    typeof value.ivBase64 === "string" &&
    typeof value.ciphertextBase64 === "string"
  );
}

function validProtocolAddress(value: unknown): value is ProtocolDeploymentAddress {
  return (
    isExactRecord(value, ["tenant", "functionName", "version"]) &&
    validTenant(value.tenant) &&
    typeof value.functionName === "string" &&
    /^[a-z][a-z0-9-]{0,62}$/u.test(value.functionName) &&
    boundedInteger(value.version, 1, Number.MAX_SAFE_INTEGER)
  );
}

function validFunctionAddress(value: unknown): value is FunctionAddress {
  return (
    isExactRecord(value, ["tenant", "functionName"]) &&
    validTenant(value.tenant) &&
    typeof value.functionName === "string" &&
    /^[a-z][a-z0-9-]{0,62}$/u.test(value.functionName)
  );
}

function validTenant(value: unknown): value is Tenant {
  return (
    isExactRecord(value, ["projectId", "environmentId"]) &&
    validProjectId(value.projectId) &&
    validEnvironmentId(value.environmentId)
  );
}

function validProjectId(value: unknown): value is string {
  return typeof value === "string" && /^prj_[A-Za-z0-9_-]{8,64}$/u.test(value);
}

function validEnvironmentId(value: unknown): value is string {
  return typeof value === "string" && /^env_[A-Za-z0-9_-]{8,64}$/u.test(value);
}

function validRelativePath(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= 512 &&
    !value.startsWith("/") &&
    !value.includes("\\") &&
    !hasControl(value) &&
    value.split("/").every((part) => part !== "" && part !== "." && part !== "..")
  );
}

function validRegion(value: string): boolean {
  return /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/u.test(value);
}

function validRequestId(value: unknown): value is string {
  return typeof value === "string" && /^req_[A-Za-z0-9_-]{8,128}$/u.test(value);
}

function validHeaderPairs(value: unknown): value is [string, string][] {
  return (
    Array.isArray(value) &&
    value.length <= MAX_RESPONSE_HEADERS &&
    value.every(
      (pair) =>
        Array.isArray(pair) &&
        pair.length === 2 &&
        typeof pair[0] === "string" &&
        typeof pair[1] === "string" &&
        /^[!#$%&'*+.^_`|~A-Za-z0-9-]{1,128}$/u.test(pair[0]) &&
        pair[1].length <= 16 * 1024 &&
        !hasControl(pair[1]),
    )
  );
}

function safeRequestHeaders(value: [string, string][]): Headers {
  const headers = new Headers();
  for (const [name, content] of value) {
    const lower = name.toLowerCase();
    if (
      [
        "connection",
        "content-length",
        "host",
        "transfer-encoding",
        WORKER_LOG_HEADER,
        AUTHORIZATION_HEADER,
        VERSION_HEADER,
        REQUEST_ID_HEADER,
      ].includes(lower) ||
      lower.startsWith("x-mako-runtime-")
    )
      continue;
    headers.append(name, content);
  }
  return headers;
}

function safeResponseHeaders(headers: Headers): [string, string][] {
  const output: [string, string][] = [];
  for (const [name, value] of headers) {
    const lower = name.toLowerCase();
    if (
      [
        "connection",
        "content-length",
        "host",
        "transfer-encoding",
        WORKER_LOG_HEADER,
        AUTHORIZATION_HEADER,
        VERSION_HEADER,
        REQUEST_ID_HEADER,
      ].includes(lower)
    )
      continue;
    if (output.length >= MAX_RESPONSE_HEADERS || value.length > 16 * 1024 || hasControl(value))
      throw new Error("invalid worker response header");
    output.push([name, value]);
  }
  return output;
}

function stripRuntimeHeaders(headers: Headers): void {
  for (const name of [...headers.keys()]) {
    if (name.toLowerCase().startsWith("x-mako-runtime-")) headers.delete(name);
  }
  headers.delete(AUTHORIZATION_HEADER);
  headers.delete(VERSION_HEADER);
  headers.delete("content-length");
  headers.delete("host");
}

async function fileExists(path: string): Promise<boolean> {
  try {
    return (await Deno.stat(path)).isFile;
  } catch {
    return false;
  }
}

/// The captured console lines a worker shipped on its response, decoded and
/// bounded. Anything malformed is dropped rather than trusted: this header
/// crosses an isolate boundary from customer code.
function shippedWorkerLogs(headers: Headers): { level: DeploymentLog["level"]; message: string }[] {
  const encoded = headers.get(WORKER_LOG_HEADER);
  if (encoded === null || encoded.length > MAX_SHIPPED_LOG_BYTES * 2) return [];
  let parsed: unknown;
  try {
    const bytes = decodeBase64(encoded, MAX_SHIPPED_LOG_BYTES);
    parsed = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const lines: { level: DeploymentLog["level"]; message: string }[] = [];
  for (const value of parsed.slice(0, MAX_SHIPPED_LOG_LINES)) {
    if (typeof value !== "object" || value === null) continue;
    const entry = value as { level?: unknown; message?: unknown };
    const raw = typeof entry.message === "string" ? entry.message.slice(0, 4_096) : "";
    // Control characters would fail the log page's own wire validation and
    // wedge collection for the function; spaces keep the line readable.
    let flattened = "";
    for (const character of raw) {
      flattened += character < " " || character === "\u007f" ? " " : character;
    }
    const message = flattened.trim();
    if (message.length === 0) continue;
    const level = ["debug", "info", "warn", "error"].includes(String(entry.level))
      ? (entry.level as DeploymentLog["level"])
      : "info";
    lines.push({ level, message });
  }
  return lines;
}

/// The capture shared by both shims: console patched to remember what a
/// request printed, and a function that ships the lines on a response.
function captureSource(): string {
  return `type CapturedLine = { level: string; message: string };
const captured: CapturedLine[] = [];
const render = (value: unknown): string => {
  if (typeof value === "string") return value;
  if (value instanceof Error) return \`\${value.name}: \${value.message}\`;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
};
const patch = (level: string, original: (...values: unknown[]) => void) => {
  return (...values: unknown[]) => {
    if (captured.length < ${MAX_SHIPPED_LOG_LINES}) {
      captured.push({ level, message: values.map(render).join(" ").slice(0, 4096) });
    }
    original(...values);
  };
};
console.log = patch("info", console.log.bind(console));
console.info = patch("info", console.info.bind(console));
console.warn = patch("warn", console.warn.bind(console));
console.error = patch("error", console.error.bind(console));
console.debug = patch("debug", console.debug.bind(console));
// No reset per request: concurrent requests in this isolate share the array,
// and a reset would wipe a neighbour's captured lines. Draining at ship time
// means a line lands on whichever response ships next -- same function, same
// tenant, at worst blurred attribution.
const ship = (response: Response): Response => {
  if (captured.length === 0) return response;
  try {
    const bytes = new TextEncoder().encode(JSON.stringify(captured.splice(0)));
    if (bytes.length > ${MAX_SHIPPED_LOG_BYTES}) return response;
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    const headers = new Headers(response.headers);
    headers.set("${WORKER_LOG_HEADER}", btoa(binary));
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    });
  } catch {
    return response;
  }
};
`;
}

/// The shim for a module that registers its handler with Deno.serve as it
/// loads: Deno.serve is wrapped first, then the module is imported, so the
/// handler it passes -- in any of Deno.serve's call shapes -- ships its lines.
function serveShimSource(entrypoint: string): string {
  return `// Written by the Mako runtime supervisor. Captures this function's console
// output per request so the platform can retain it; the header it ships on
// never leaves the runtime.
${captureSource()}
type Handler = (request: Request, info: unknown) => Response | Promise<Response>;
const wrap = (handler: Handler): Handler => async (request, info) => ship(await handler(request, info));
const serve = Deno.serve.bind(Deno) as (...args: unknown[]) => unknown;
// deno-lint-ignore no-explicit-any
(Deno as any).serve = (...args: unknown[]) => {
  const [first, second] = args;
  if (typeof first === "function") return serve(wrap(first as Handler), ...args.slice(1));
  if (typeof second === "function") return serve(first, wrap(second as Handler), ...args.slice(2));
  if (first !== null && typeof first === "object" && typeof (first as { handler?: unknown }).handler === "function") {
    const options = first as { handler: Handler };
    return serve({ ...options, handler: wrap(options.handler) }, ...args.slice(1));
  }
  return serve(...args);
};
await import("./${entrypoint}");
`;
}

/// The module written next to a user bundle's own files. It patches console
/// to remember what a request printed, forwards every line to the real
/// console so container logs stay whole, and ships the captured lines back
/// on the internal header the supervisor strips. Capture must never break
/// the function: every failure path returns the user's response untouched.
function consoleShimSource(entrypoint: string): string {
  return `// Written by the Mako runtime supervisor. Captures this function's console
// output per request so the platform can retain it; the header it ships on
// never leaves the runtime.
import user from "./${entrypoint}";
${captureSource()}
export default {
  async fetch(request: Request): Promise<Response> {
    return ship(await user.fetch(request));
  },
};
`;
}

function appendLog(
  record: DeploymentRecord,
  level: DeploymentLog["level"],
  message: string,
  correlationId: string,
  region: string,
): void {
  record.logs.push({
    deployment: record.load.manifest.deployment,
    timestampUnixMilliseconds: Date.now(),
    level,
    message,
    correlationId,
    region,
  });
  if (record.logs.length > MAX_LOGS_PER_DEPLOYMENT)
    record.logs.splice(0, record.logs.length - MAX_LOGS_PER_DEPLOYMENT);
  // The entry count alone does not bound the persisted state: shipped lines
  // carry customer text, and a state file that outgrows the decrypt ceiling
  // would refuse to restore on the next boot. Evict oldest until the total
  // retained text fits a budget the ceiling comfortably covers.
  let retained = 0;
  for (const entry of record.logs) retained += entry.message.length;
  while (retained > MAX_RETAINED_LOG_CHARS && record.logs.length > 1) {
    const evicted = record.logs.shift();
    if (evicted === undefined) break;
    retained -= evicted.message.length;
  }
}

function flattenAddress(value: ProtocolDeploymentAddress): DeploymentAddress {
  return { ...value.tenant, functionName: value.functionName, version: value.version };
}

function deploymentKey(value: DeploymentAddress): string {
  return `${value.projectId}/${value.environmentId}/${value.functionName}/${value.version}`;
}

function workerDirectory(root: string, value: DeploymentAddress): string {
  return `${root}/${value.projectId}/${value.environmentId}/${value.functionName}/${value.version}`;
}

function sameFunction(
  deployment: ProtocolDeploymentAddress,
  functionAddress: FunctionAddress,
): boolean {
  return (
    deployment.functionName === functionAddress.functionName &&
    deployment.tenant.projectId === functionAddress.tenant.projectId &&
    deployment.tenant.environmentId === functionAddress.tenant.environmentId
  );
}

function sameFlatFunction(
  deployment: DeploymentAddress,
  functionAddress: FunctionAddress,
): boolean {
  return (
    deployment.functionName === functionAddress.functionName &&
    deployment.projectId === functionAddress.tenant.projectId &&
    deployment.environmentId === functionAddress.tenant.environmentId
  );
}

function deploymentStatus(
  deployment: ProtocolDeploymentAddress,
  state: "healthy" | "retired",
  region: string,
  requestId: string,
): Response {
  return runtimeJson(
    200,
    { protocolVersion: PROTOCOL_VERSION, deployment, state, region, diagnosticCode: null },
    requestId,
  );
}

function runtimeError(
  status: number,
  code: string,
  requestId: string,
  retryable: boolean,
): Response {
  return runtimeJson(
    status,
    { protocolVersion: PROTOCOL_VERSION, code, requestId, retryable },
    requestId,
  );
}

function runtimeJson(status: number, value: unknown, requestId: string): Response {
  const body = JSON.stringify(value);
  return new Response(body, {
    status,
    headers: {
      "content-type": "application/json",
      "content-length": String(new TextEncoder().encode(body).length),
      [REQUEST_ID_HEADER]: requestId,
      [VERSION_HEADER]: String(PROTOCOL_VERSION),
    },
  });
}

function publicFailure(
  status: number,
  code: "invalid_request" | "not_found" | "internal",
  message: string,
  requestId: string,
): Response {
  return Response.json(
    { apiVersion: "v1", error: { code, message, requestId, retry: { kind: "never" } } },
    { status, headers: { "x-mako-request-id": requestId } },
  );
}

async function boundedBody(request: Request, maximum: number): Promise<Uint8Array> {
  const declared = request.headers.get("content-length");
  if (declared !== null && (!/^[0-9]+$/u.test(declared) || Number(declared) > maximum))
    throw new Error("body limit exceeded");
  const body = new Uint8Array(await request.arrayBuffer());
  if (body.length > maximum) throw new Error("body limit exceeded");
  return body;
}

function decodeBase64(value: string, maximum: number): Uint8Array {
  if (
    value.length > Math.ceil(maximum / 3) * 4 + 4 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(value)
  )
    throw new Error("invalid base64");
  const decoded = atob(value);
  if (decoded.length > maximum) throw new Error("decoded value exceeds limit");
  const bytes = new Uint8Array(decoded.length);
  for (let index = 0; index < decoded.length; index += 1) bytes[index] = decoded.charCodeAt(index);
  return bytes;
}

function encodeBase64(value: Uint8Array): string {
  let output = "";
  for (let offset = 0; offset < value.length; offset += 0x8000)
    output += String.fromCharCode(...value.subarray(offset, offset + 0x8000));
  return btoa(output);
}

function decodeHex(value: string): Uint8Array {
  if (!/^[0-9a-f]+$/u.test(value) || value.length % 2 !== 0) return new Uint8Array();
  const output = new Uint8Array(value.length / 2);
  for (let index = 0; index < output.length; index += 1)
    output[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  return output;
}

async function sha256Digest(value: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytesBuffer(value)));
  return `sha256:${hex(digest)}`;
}

async function sha256Hex(value: string): Promise<string> {
  return hex(
    new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value))),
  );
}

async function requestDigest(value: SensitiveLoad): Promise<string> {
  return await sha256Hex(
    JSON.stringify({
      manifest: value.manifest,
      bundleBase64: value.bundleBase64,
      secrets: value.secrets,
    }),
  );
}

function hex(value: Uint8Array): string {
  return [...value].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function bytesBuffer(value: Uint8Array): ArrayBuffer {
  return value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength) as ArrayBuffer;
}

async function constantTimeEqual(candidate: string | null, expected: string): Promise<boolean> {
  const [left, right] = await Promise.all([sha256Hex(candidate ?? ""), sha256Hex(expected)]);
  let difference = candidate === null ? 1 : 0;
  for (let index = 0; index < left.length; index += 1)
    difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}

function rewriteImports(source: string, importer: string, imports: Record<string, string>): string {
  let rewritten = source;
  for (const [specifier, target] of Object.entries(imports)) {
    // Resolved by the worker's import map, not by rewriting the source.
    if (target === SDK_SPECIFIER) continue;
    const relative = relativeImport(importer, target);
    rewritten = rewritten
      .replaceAll(`"${specifier}"`, `"${relative}"`)
      .replaceAll(`'${specifier}'`, `'${relative}'`);
  }
  return rewritten;
}

/**
 * The hosts one worker may open a connection to, as Deno `--allow-net`
 * entries. There is no unrestricted grant and no empty list here: an empty
 * list is how Deno spells "every host", which is what this list existing at
 * all is meant to prevent.
 *
 * `deny_all` denies the *function's* own destinations. It does not deny the
 * platform's API origin, which the runtime injects as `MAKO_API_URL` and the
 * first-party SDK is built to call: a function that could not reach it could
 * not read or write a document, which is the reason hosted functions exist.
 * An `allow_list` policy adds the deployment's declared hosts, each pinned to
 * port 443, so granting a host never grants its neighbours on other ports. The
 * variant carries hosts and nothing else -- a per-invocation request count
 * cannot be enforced from inside an isolate the tenant controls, so the
 * protocol no longer offers one.
 */
export function networkGrants(policy: OutboundNetwork): string[] {
  const grants = [originGrant(requiredEnvironment("MAKO_API_URL"))];
  if (policy.mode === "allow_list") {
    for (const host of policy.hosts) {
      if (!validOutboundHost(host)) throw new Error("unsupported outbound network policy");
      grants.push(`${host}:443`);
    }
  }
  return grants;
}

/**
 * One `host:port` grant for an absolute origin. The port is always explicit,
 * so granting an API host does not also grant every other service listening on
 * the same machine.
 */
export function originGrant(value: string): string {
  const url = new URL(value);
  if ((url.protocol !== "http:" && url.protocol !== "https:") || url.hostname === "") {
    throw new Error("invalid platform origin");
  }
  const port = url.port !== "" ? url.port : url.protocol === "https:" ? "443" : "80";
  return `${url.hostname}:${port}`;
}

/**
 * The worker's import map, inline. `data:{encodeURIComponent(json)}?{base}` is
 * the form the runtime accepts, and the base directory is the worker's own, so
 * the one mapping resolves to a file the worker is allowed to read.
 */
function firstPartyImportMap(directory: string): string {
  const map = { imports: { [SDK_SPECIFIER]: `./${SDK_MODULE}` } };
  return `data:${encodeURIComponent(JSON.stringify(map))}?${encodeURIComponent(directory)}`;
}

function relativeImport(importer: string, target: string): string {
  const from = importer.split("/").slice(0, -1);
  const to = target.split("/");
  while (from[0] !== undefined && from[0] === to[0]) {
    from.shift();
    to.shift();
  }
  const value = `${"../".repeat(from.length)}${to.join("/")}`;
  return value.startsWith(".") ? value : `./${value}`;
}

function decodeCursor(value: string | null, length: number): number | null {
  if (value === null) return 0;
  const match = /^runtime1\.([0-9]+)$/u.exec(value);
  if (match?.[1] === undefined) return null;
  const offset = Number(match[1]);
  return Number.isSafeInteger(offset) && offset <= length ? offset : null;
}

function referenceKey(value: SecretReference): string {
  return `${value.name}:${value.version}`;
}

function boundedInteger(value: unknown, minimum: number, maximum: number): value is number {
  return (
    typeof value === "number" && Number.isSafeInteger(value) && value >= minimum && value <= maximum
  );
}

function hasControl(value: string): boolean {
  return [...value].some(
    (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
  );
}

function requiredEnvironment(name: string): string {
  const value = Deno.env.get(name);
  if (value === undefined || value === "") throw new Error(`missing runtime setting: ${name}`);
  return value;
}

function requestIdentifier(candidate: string | null): string {
  return candidate !== null && validRequestId(candidate)
    ? candidate
    : `req_${crypto.randomUUID().replaceAll("-", "")}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isExactRecord(value: unknown, keys: string[]): value is Record<string, unknown> {
  return (
    isRecord(value) &&
    Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key))
  );
}
