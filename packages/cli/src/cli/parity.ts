import type { Command } from "./registry.js";

/**
 * Path prefixes and shapes outside the developer surface. Operator routes carry
 * their own identity; application-runtime routes are what SDKs and RxDB clients
 * call with project credentials, not what a developer does at a console.
 */
export const EXCLUDED_PATH_PREFIXES: readonly string[] = [
  "/v1/operator",
  "/v1/operator-auth",
  "/{projectRef}/functions",
];

export const EXCLUDED_PATH_PATTERNS: readonly RegExp[] = [
  /\/environments\/\{environmentId\}\/auth\//u,
  /\/collections\/\{collectionId\}\/documents(?:\/|$)/u,
  /\/environments\/\{environmentId\}\/service\//u,
  /\/collections\/\{collectionId\}\/replication\//u,
  // Object upload, download, and listing: what an application does with a session.
  /\/environments\/\{environmentId\}\/storage\//u,
];

export interface OpenApiLike {
  readonly paths: Readonly<Record<string, Readonly<Record<string, unknown>>>>;
}

export function isDeveloperFacingPath(path: string): boolean {
  if (EXCLUDED_PATH_PREFIXES.some((prefix) => path.startsWith(prefix))) return false;
  return !EXCLUDED_PATH_PATTERNS.some((pattern) => pattern.test(path));
}

/** Every operation id a developer can reach through the console, from the API document. */
export function developerFacingOperations(document: OpenApiLike): readonly string[] {
  const ids: string[] = [];
  for (const [path, item] of Object.entries(document.paths)) {
    if (!isDeveloperFacingPath(path)) continue;
    for (const [method, operation] of Object.entries(item)) {
      if (method === "parameters" || typeof operation !== "object" || operation === null) continue;
      const id = (operation as { operationId?: unknown }).operationId;
      if (typeof id === "string") ids.push(id);
    }
  }
  return ids.sort();
}

/** Which commands reach each operation id. */
export function commandOperations(
  commands: readonly Command[],
): ReadonlyMap<string, readonly string[]> {
  const map = new Map<string, string[]>();
  for (const command of commands) {
    for (const operation of command.operations) {
      const list = map.get(operation) ?? [];
      list.push(command.path.join(" "));
      map.set(operation, list);
    }
  }
  return map;
}
