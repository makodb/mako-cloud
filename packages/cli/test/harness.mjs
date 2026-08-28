// Test harness for the CLI: a loopback mock of the management API and an
// in-process runner that captures stdout, stderr, and the exit code.
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Readable } from "node:stream";

import { commands as allCommands, run } from "../dist/index.js";

export const REFRESH_COOKIE = "__Host-mako_developer_refresh";
export const NOW = "2026-08-06T12:00:00.000Z";
export const TEAM_ID = "org_abcdefgh";
export const PERSONAL_TEAM_ID = "org_personal";
export const PROJECT_ID = "prj_abcdefgh";
export const ENVIRONMENT_ID = "env_abcdefgh";

/** An API error body as the control plane sends it. */
export function apiError(code, message, status, retry = { kind: "never" }) {
  return { status, json: { apiVersion: "v1", error: { code, message, requestId: "req_abcdefgh", retry } } };
}

/**
 * Starts a mock API. `handler(request)` receives `{method, path, query, headers,
 * body, text}` and returns `{status, json?, text?, headers?}` or `undefined` for
 * a 404 that is also recorded under `unhandled`.
 */
export async function startMockApi(handler) {
  const requests = [];
  const unhandled = [];
  const server = createServer(async (incoming, outgoing) => {
    const chunks = [];
    for await (const chunk of incoming) chunks.push(chunk);
    const text = Buffer.concat(chunks).toString("utf8");
    const url = new URL(incoming.url ?? "/", "http://127.0.0.1");
    let body;
    try {
      body = text === "" ? undefined : JSON.parse(text);
    } catch {
      body = undefined;
    }
    const request = {
      method: incoming.method ?? "GET",
      path: url.pathname,
      query: Object.fromEntries(url.searchParams.entries()),
      headers: incoming.headers,
      body,
      text,
    };
    requests.push(request);
    let response;
    try {
      response = await handler(request);
    } catch (error) {
      response = apiError("internal", String(error), 500);
    }
    if (response === undefined) {
      unhandled.push(`${request.method} ${request.path}`);
      response = apiError("not_found", `no handler for ${request.method} ${request.path}`, 404);
    }
    const headers = { ...(response.headers ?? {}) };
    let payload = "";
    if (response.json !== undefined) {
      headers["content-type"] = "application/json";
      payload = JSON.stringify(response.json);
    } else if (response.text !== undefined) {
      headers["content-type"] = headers["content-type"] ?? "text/plain";
      payload = response.text;
    }
    outgoing.writeHead(response.status ?? 200, headers);
    outgoing.end(payload);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return {
    endpoint: `http://127.0.0.1:${port}`,
    requests,
    unhandled,
    /** Requests for one path, optionally one method. */
    find: (path, method) =>
      requests.filter((r) => r.path === path && (method === undefined || r.method === method)),
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

/** Runs the CLI in-process with captured streams; `stdin` is a string or null. */
export async function runCli(argv, options = {}) {
  let stdout = "";
  let stderr = "";
  const io = {
    stdout: { write: (chunk) => (stdout += String(chunk)) },
    stderr: { write: (chunk) => (stderr += String(chunk)) },
    stdin: options.stdin === undefined || options.stdin === null ? null : Readable.from([options.stdin]),
    isTTY: options.isTTY ?? false,
    env: { HOME: options.configDir ?? tmpdir(), MAKO_CONFIG_DIR: options.configDir, ...(options.env ?? {}) },
    cwd: process.cwd(),
  };
  const code = await run(argv, io, options.commands ?? allCommands);
  return { code, stdout, stderr };
}

/** A fresh, private config directory removed when the test ends. */
export async function configDir(t) {
  const directory = await mkdtemp(join(tmpdir(), "mako-cli-test-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

export function session(overrides = {}) {
  return {
    accessToken: "developer-session-token-0001",
    tokenType: "Bearer",
    audience: "mako-management",
    status: "active",
    expiresAt: "2099-01-01T00:00:00.000Z",
    ...overrides,
  };
}

export function team(overrides = {}) {
  return {
    id: TEAM_ID,
    name: "Acme",
    kind: "team",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

export function personalTeam() {
  return team({ id: PERSONAL_TEAM_ID, name: "Your projects", kind: "personal" });
}

export function project(overrides = {}) {
  return {
    id: PROJECT_ID,
    teamId: TEAM_ID,
    name: "Mako Test Project",
    region: "us-east-1",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

export function environment(overrides = {}) {
  return {
    id: ENVIRONMENT_ID,
    projectId: PROJECT_ID,
    name: "development",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

/** Signs a profile in against the mock so later commands carry a bearer token. */
export async function signedIn(t, api, extra = {}) {
  const directory = await configDir(t);
  const result = await runCli(
    ["auth", "login", "--endpoint", api.endpoint, "--email", "owner@example.test", "--password-file", await passwordFile(directory)],
    { configDir: directory, ...extra },
  );
  if (result.code !== 0) throw new Error(`login failed: ${result.stderr}`);
  return directory;
}

export async function passwordFile(directory) {
  const { writeFile } = await import("node:fs/promises");
  const path = join(directory, "password.txt");
  await writeFile(path, "correct horse battery staple\n", { mode: 0o600 });
  return path;
}

/** A handler that answers sign-in, refresh, sign-out, and team listing. */
export function authHandler(options = {}) {
  const teams = options.teams ?? [personalTeam(), team()];
  return (request) => {
    if (request.method === "POST" && request.path === "/v1/developer-auth/sessions") {
      return {
        status: 200,
        json: session(options.session ?? {}),
        headers: { "set-cookie": `${REFRESH_COOKIE}=refresh-cookie-0001; Path=/; Secure; HttpOnly; SameSite=Strict` },
      };
    }
    if (request.method === "POST" && request.path === "/v1/developer-auth/sessions/refresh") {
      return {
        status: 200,
        json: session({ accessToken: "developer-session-token-0002" }),
        headers: { "set-cookie": `${REFRESH_COOKIE}=refresh-cookie-0002; Path=/; Secure; HttpOnly; SameSite=Strict` },
      };
    }
    if (request.method === "DELETE" && request.path === "/v1/developer-auth/sessions/current") {
      return { status: 204 };
    }
    if (request.method === "GET" && request.path === "/v1/teams") {
      return { status: 200, json: { items: teams } };
    }
    return options.fallback?.(request);
  };
}
