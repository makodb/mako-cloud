// Mako main worker for Supabase Edge Runtime v1.74.3.
// This file runs inside the pinned runtime image and intentionally has no
// network imports, so the runtime contract never depends on a moving resource.

import { originGrant, RuntimeSupervisor } from "./supervisor.ts";

type UserWorker = {
  fetch(request: Request, options: { signal: AbortSignal }): Promise<Response>;
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
      // An empty list is Deno's spelling of "granted without restriction";
      // `null` is the absence of a grant. Nothing here may be `[]`.
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
  serve(
    options: { readonly port: number; readonly hostname: string },
    handler: (request: Request) => Response | Promise<Response>,
  ): void;
};

const projectId = requiredEnvironment("MAKO_PROJECT_ID");
const environmentId = requiredEnvironment("MAKO_ENVIRONMENT_ID");
const functionName = requiredEnvironment("MAKO_FUNCTION_NAME");
const functionPath = requiredEnvironment("MAKO_FUNCTION_PATH");
const entrypoint = requiredEnvironment("MAKO_ENTRYPOINT");
const verifyJwtByDefault = requiredEnvironment("MAKO_VERIFY_JWT") === "true";
const wallTimeMilliseconds = boundedIntegerEnvironment("MAKO_WALL_TIME_MS", 100, 300_000);
const jwtIssuer = Deno.env.get("MAKO_JWT_ISSUER") ?? "";
const jwtAudience = Deno.env.get("MAKO_JWT_AUDIENCE") ?? "";
const jwks = parseJwks(Deno.env.get("MAKO_JWKS") ?? "");
const userEnvironment = selectedUserEnvironment(requiredEnvironment("MAKO_USER_ENV_NAMES"));
const stablePrefix = `/${projectId}/functions/v1/${functionName}`;
const allowedMethods = new Set(["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]);
const runtimeSupervisor = await RuntimeSupervisor.open();

Deno.serve({ port: 9000, hostname: "0.0.0.0" }, async (request: Request) => {
  if (new URL(request.url).pathname.startsWith("/_mako/runtime/")) {
    return runtimeSupervisor.manage(request);
  }
  const supervised = await runtimeSupervisor.tryInvoke(request);
  if (supervised !== null) return supervised;
  const requestId = requestIdentifier(request.headers.get("x-request-id"));
  if (!allowedMethods.has(request.method)) {
    return failure(405, "invalid_request", "function method is not supported", requestId);
  }

  let path: string;
  try {
    path = functionOwnedPath(new URL(request.url).pathname);
  } catch {
    return failure(400, "invalid_request", "function request is invalid", requestId);
  }
  if (path === "") {
    return failure(404, "not_found", "function was not found", requestId);
  }

  const authorization = request.headers.get("authorization");
  let callerToken: string | null = null;
  if (verifyJwtByDefault || authorization !== null) {
    try {
      callerToken = await verifyAuthorization(authorization);
    } catch {
      return failure(401, "unauthenticated", "application-user session is invalid", requestId);
    }
  }

  const traceId = traceIdentifier(request.headers.get("traceparent"));
  const headers = new Headers(request.headers);
  for (const name of [...headers.keys()]) {
    if (name.toLowerCase().startsWith("x-mako-")) headers.delete(name);
  }
  headers.delete("authorization");
  headers.set("x-mako-request-id", requestId);
  headers.set("x-mako-trace-id", traceId);
  if (callerToken !== null) headers.set("x-mako-caller-authorization", callerToken);

  const target = new URL(request.url);
  target.pathname = path;
  const forwarded = new Request(target, {
    method: request.method,
    headers,
    body: request.method === "GET" || request.method === "HEAD" ? null : request.body,
  });
  EdgeRuntime.applySupabaseTag(request, forwarded);
  const controller = new AbortController();
  request.signal.addEventListener("abort", () => controller.abort(), { once: true });
  try {
    const worker = await createWorker(requestId, traceId);
    const response = await worker.fetch(forwarded, { signal: controller.signal });
    const responseHeaders = new Headers(response.headers);
    responseHeaders.set("x-mako-request-id", requestId);
    return new Response(request.method === "HEAD" ? null : response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: responseHeaders,
    });
  } catch {
    return failure(500, "internal", "function invocation failed", requestId);
  }
});

function createWorker(requestId: string, traceId: string): Promise<UserWorker> {
  const environment = {
    ...userEnvironment,
    MAKO_API_URL: requiredEnvironment("MAKO_API_URL"),
    MAKO_PROJECT_ID: projectId,
    MAKO_ENVIRONMENT_ID: environmentId,
    MAKO_FUNCTION_NAME: functionName,
  };
  return EdgeRuntime.userWorkers.create({
    servicePath: functionPath,
    maybeEntrypoint: `file://${functionPath}/${entrypoint}`,
    memoryLimitMb: 150,
    workerTimeoutMs: wallTimeMilliseconds,
    noModuleCache: false,
    envVars: Object.entries(environment),
    forceCreate: false,
    cpuTimeSoftLimitMs: 10_000,
    cpuTimeHardLimitMs: 20_000,
    staticPatterns: [`${functionPath}/**/*.wasm`],
    // The same grants a hosted deployment gets, so a function that works
    // locally is not one the hosted sandbox will refuse: its own directory,
    // the environment names it was given, the platform API origin, and
    // nothing else. Denied capabilities are `null` -- an empty list would
    // grant them without restriction.
    permissions: {
      allow_all: false,
      allow_env: Object.keys(environment),
      allow_net: [originGrant(requiredEnvironment("MAKO_API_URL"))],
      allow_read: [functionPath],
      allow_write: null,
      allow_import: null,
      allow_run: null,
      allow_ffi: null,
      allow_sys: null,
    },
    context: {
      runtimeProtocol: 1,
      projectId,
      environmentId,
      functionName,
      requestId,
      traceId,
      mode: "local",
    },
  });
}

function requiredEnvironment(name: string): string {
  const value = Deno.env.get(name);
  if (value === undefined || value === "") throw new Error(`missing runtime setting: ${name}`);
  return value;
}

function boundedIntegerEnvironment(name: string, minimum: number, maximum: number): number {
  const value = Number(requiredEnvironment(name));
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new Error(`invalid runtime setting: ${name}`);
  }
  return value;
}

function selectedUserEnvironment(source: string): Record<string, string> {
  const names: unknown = JSON.parse(source);
  if (!Array.isArray(names) || names.length > 256) throw new Error("invalid user environment map");
  const selected: Record<string, string> = {};
  for (const name of names) {
    if (
      typeof name !== "string" ||
      !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/u.test(name) ||
      name === "DENO_DIR" ||
      name.startsWith("MAKO_") ||
      name.startsWith("EDGE_RUNTIME_") ||
      Object.hasOwn(selected, name)
    ) {
      throw new Error("invalid user environment name");
    }
    const value = Deno.env.get(name);
    if (value === undefined) throw new Error("missing selected user environment value");
    selected[name] = value;
  }
  return selected;
}

function functionOwnedPath(pathname: string): string {
  if (pathname !== stablePrefix && !pathname.startsWith(`${stablePrefix}/`)) return "";
  const suffix = pathname.slice(stablePrefix.length) || "/";
  if (
    suffix !== "/" &&
    (suffix.length > 8_192 ||
      suffix.includes("\\") ||
      suffix
        .split("/")
        .slice(1)
        .some((segment) => segment === "" || segment === "." || segment === ".."))
  ) {
    throw new Error("invalid path");
  }
  return suffix;
}

type Ed25519JsonWebKey = JsonWebKey & { readonly kid: string };
type JsonWebKeySet = { readonly keys: readonly Ed25519JsonWebKey[] };

function parseJwks(source: string): JsonWebKeySet | null {
  if (source === "") return null;
  const parsed: unknown = JSON.parse(source);
  if (!isRecord(parsed) || !Array.isArray(parsed.keys)) throw new Error("invalid JWKS");
  return { keys: parsed.keys as Ed25519JsonWebKey[] };
}

async function verifyAuthorization(authorization: string | null): Promise<string> {
  if (authorization === null || authorization.length > 16_391) throw new Error("missing token");
  const match = /^Bearer ([A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)$/u.exec(authorization);
  if (match?.[1] === undefined || jwks === null || jwtIssuer === "" || jwtAudience === "") {
    throw new Error("invalid token");
  }
  const token = match[1];
  const [encodedHeader, encodedPayload, encodedSignature] = token.split(".");
  if (
    encodedHeader === undefined ||
    encodedPayload === undefined ||
    encodedSignature === undefined
  ) {
    throw new Error("invalid token");
  }
  const header: unknown = JSON.parse(new TextDecoder().decode(base64UrlDecode(encodedHeader)));
  const claims: unknown = JSON.parse(new TextDecoder().decode(base64UrlDecode(encodedPayload)));
  if (
    !isRecord(header) ||
    header.alg !== "EdDSA" ||
    (header.typ !== undefined && header.typ !== "JWT") ||
    typeof header.kid !== "string" ||
    !isRecord(claims)
  ) {
    throw new Error("invalid token");
  }
  const jwk = jwks.keys.find(
    (candidate) =>
      candidate.kid === header.kid &&
      candidate.kty === "OKP" &&
      candidate.crv === "Ed25519" &&
      candidate.alg === "EdDSA" &&
      (candidate.use === undefined || candidate.use === "sig"),
  );
  if (jwk === undefined) throw new Error("unknown token key");
  const key = await crypto.subtle.importKey("jwk", jwk, { name: "Ed25519" }, false, ["verify"]);
  const validSignature = await crypto.subtle.verify(
    "Ed25519",
    key,
    base64UrlDecode(encodedSignature),
    new TextEncoder().encode(`${encodedHeader}.${encodedPayload}`),
  );
  if (!validSignature) throw new Error("invalid token signature");

  const now = Math.floor(Date.now() / 1_000);
  if (
    claims.iss !== jwtIssuer ||
    !matchesAudience(claims.aud, jwtAudience) ||
    claims.project_id !== projectId ||
    claims.environment_id !== environmentId ||
    typeof claims.sub !== "string" ||
    claims.sub.length < 1 ||
    !Number.isSafeInteger(claims.iat) ||
    !Number.isSafeInteger(claims.exp) ||
    (claims.iat as number) > now + 30 ||
    (claims.exp as number) <= now - 30
  ) {
    throw new Error("invalid token claims");
  }
  return token;
}

function matchesAudience(value: unknown, expected: string): boolean {
  return value === expected || (Array.isArray(value) && value.includes(expected));
}

function base64UrlDecode(value: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]+$/u.test(value)) throw new Error("invalid base64url");
  const padded = value
    .replaceAll("-", "+")
    .replaceAll("_", "/")
    .padEnd(Math.ceil(value.length / 4) * 4, "=");
  const decoded = atob(padded);
  const bytes = new Uint8Array(decoded.length);
  for (let index = 0; index < decoded.length; index += 1) {
    bytes[index] = decoded.charCodeAt(index);
  }
  return bytes;
}

function requestIdentifier(candidate: string | null): string {
  return candidate !== null && /^req_[A-Za-z0-9_-]{8,128}$/u.test(candidate)
    ? candidate
    : `req_${crypto.randomUUID().replaceAll("-", "")}`;
}

function traceIdentifier(traceparent: string | null): string {
  const traceId =
    traceparent === null
      ? undefined
      : /^[\da-f]{2}-([\da-f]{32})-[\da-f]{16}-[\da-f]{2}$/iu.exec(traceparent)?.[1];
  return traceId?.toLowerCase() ?? crypto.randomUUID().replaceAll("-", "");
}

function failure(
  status: number,
  code: "invalid_request" | "unauthenticated" | "not_found" | "internal",
  message: string,
  requestId: string,
): Response {
  return Response.json(
    {
      apiVersion: "v1",
      error: { code, message, requestId, retry: { kind: "never" } },
    },
    { status, headers: { "x-mako-request-id": requestId } },
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
