import { mkdir, readFile, rename, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import { CliError, EXIT } from "./errors.js";

export const REFRESH_COOKIE_NAME = "__Host-mako_developer_refresh";

export interface StoredSession {
  readonly accessToken: string;
  readonly expiresAt: string;
  readonly audience: string;
  readonly email?: string;
  /** The value of the API's refresh cookie; the only way to renew the session. */
  readonly refreshCookie?: string;
}

export interface Profile {
  readonly endpoint: string;
  readonly session?: StoredSession;
}

export interface CredentialStore {
  readonly version: 1;
  readonly profiles: Record<string, Profile>;
}

const EMPTY: CredentialStore = { version: 1, profiles: {} };
const FILE_NAME = "credentials.json";

/** `MAKO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/mako-cloud`, else `~/.config/mako-cloud`. */
export function resolveConfigDir(env: Readonly<Record<string, string | undefined>>): string {
  const explicit = env.MAKO_CONFIG_DIR;
  if (explicit !== undefined && explicit !== "") return explicit;
  const xdg = env.XDG_CONFIG_HOME;
  const base = xdg !== undefined && xdg !== "" ? xdg : join(env.HOME ?? homedir(), ".config");
  return join(base, "mako-cloud");
}

function unsafe(path: string, why: string): CliError {
  return new CliError(
    `credential store ${path} ${why}; fix its permissions or remove it`,
    EXIT.unsafeStore,
    "CLI_UNSAFE_CREDENTIAL_STORE",
  );
}

async function assertPrivate(path: string): Promise<boolean> {
  let info: Awaited<ReturnType<typeof stat>>;
  try {
    info = await stat(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  if (!info.isFile()) throw unsafe(path, "is not a regular file");
  if (process.platform !== "win32" && (info.mode & 0o077) !== 0) {
    throw unsafe(path, "is readable by other users");
  }
  return true;
}

function isStore(value: unknown): value is CredentialStore {
  return (
    typeof value === "object" &&
    value !== null &&
    (value as { version?: unknown }).version === 1 &&
    typeof (value as { profiles?: unknown }).profiles === "object" &&
    (value as { profiles?: unknown }).profiles !== null
  );
}

/** Reads the store, refusing one other users could read. Missing means empty. */
export async function loadStore(configDir: string): Promise<CredentialStore> {
  const path = join(configDir, FILE_NAME);
  if (!(await assertPrivate(path))) return EMPTY;
  let parsed: unknown;
  try {
    parsed = JSON.parse(await readFile(path, "utf8"));
  } catch {
    throw unsafe(path, "is not valid JSON");
  }
  if (!isStore(parsed)) throw unsafe(path, "has an unknown layout");
  return parsed;
}

/** Writes the store atomically with owner-only permissions. */
export async function saveStore(configDir: string, store: CredentialStore): Promise<void> {
  await mkdir(configDir, { recursive: true, mode: 0o700 });
  const path = join(configDir, FILE_NAME);
  const temporary = `${path}.${process.pid}.tmp`;
  await writeFile(temporary, `${JSON.stringify(store, null, 2)}\n`, { mode: 0o600 });
  await rename(temporary, path);
}

export function withProfile(
  store: CredentialStore,
  name: string,
  profile: Profile | undefined,
): CredentialStore {
  const profiles = { ...store.profiles };
  if (profile === undefined) delete profiles[name];
  else profiles[name] = profile;
  return { version: 1, profiles };
}

/** Parses the refresh cookie value out of a `set-cookie` header, if present. */
export function refreshCookieFrom(setCookie: string | null | undefined): string | undefined {
  if (!setCookie) return undefined;
  for (const item of setCookie.split(/,(?=\s*[^;,=\s]+=)/u)) {
    const first = item.split(";")[0]?.trim() ?? "";
    const separator = first.indexOf("=");
    if (separator > 0 && first.slice(0, separator) === REFRESH_COOKIE_NAME) {
      return first.slice(separator + 1);
    }
  }
  return undefined;
}
