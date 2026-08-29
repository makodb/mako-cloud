import {
  MemoryAuthSessionPersistence,
  type AuthSessionPersistence,
  type PersistedAuthSession,
} from "./auth.js";

export interface BrowserAuthSessionPersistenceScope {
  readonly projectId: string;
  readonly environmentId: string;
}

export interface BrowserAuthSessionPersistenceOptions {
  /** Storage key; defaults to `mako.auth.session.<projectId>.<environmentId>`. */
  readonly key?: string;
  /**
   * The `Storage` to use instead of `globalThis.localStorage`. Pass `null` to
   * keep the session in memory only.
   */
  readonly storage?: Storage | null;
}

/**
 * Keeps the application-user session in `localStorage` so it survives a
 * reload. When storage is unavailable (a sandboxed frame, a blocked or full
 * store) the persistence degrades to memory and never throws, so sign-in
 * keeps working for the lifetime of the page.
 */
export class BrowserAuthSessionPersistence implements AuthSessionPersistence {
  readonly key: string;
  readonly #fallback = new MemoryAuthSessionPersistence();
  #storage: Storage | null;

  constructor(
    scope: BrowserAuthSessionPersistenceScope,
    options: BrowserAuthSessionPersistenceOptions = {},
  ) {
    this.key = options.key ?? `mako.auth.session.${scope.projectId}.${scope.environmentId}`;
    this.#storage = options.storage === undefined ? defaultStorage() : options.storage;
  }

  /** Whether sessions are written to `localStorage` rather than kept in memory. */
  get durable(): boolean {
    return this.#storage !== null;
  }

  async load(): Promise<PersistedAuthSession | null> {
    const storage = this.#storage;
    if (storage === null) {
      return this.#fallback.load();
    }
    let raw: string | null;
    try {
      raw = storage.getItem(this.key);
    } catch {
      this.#degrade();
      return this.#fallback.load();
    }
    if (raw === null) {
      return null;
    }
    const session = parsePersistedSession(raw);
    if (session === null) {
      this.#remove(storage);
    }
    return session;
  }

  async save(session: PersistedAuthSession): Promise<void> {
    const storage = this.#storage;
    if (storage === null) {
      await this.#fallback.save(session);
      return;
    }
    try {
      storage.setItem(this.key, JSON.stringify(session));
    } catch {
      this.#remove(storage);
      this.#degrade();
      await this.#fallback.save(session);
    }
  }

  async clear(): Promise<void> {
    await this.#fallback.clear();
    const storage = this.#storage;
    if (storage !== null) {
      this.#remove(storage);
    }
  }

  #remove(storage: Storage): void {
    try {
      storage.removeItem(this.key);
    } catch {
      this.#degrade();
    }
  }

  #degrade(): void {
    this.#storage = null;
  }
}

function defaultStorage(): Storage | null {
  try {
    const storage = (globalThis as { localStorage?: Storage | null }).localStorage;
    if (storage === undefined || storage === null) {
      return null;
    }
    const probe = `mako.auth.probe.${Date.now()}`;
    storage.setItem(probe, "1");
    storage.removeItem(probe);
    return storage;
  } catch {
    return null;
  }
}

function parsePersistedSession(raw: string): PersistedAuthSession | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isRecord(value)) {
    return null;
  }
  const { accessToken, refreshToken, expiresAtUnixMilliseconds, user } = value;
  if (
    !isNonEmptyString(accessToken) ||
    !isNonEmptyString(refreshToken) ||
    typeof expiresAtUnixMilliseconds !== "number" ||
    !Number.isFinite(expiresAtUnixMilliseconds) ||
    !isPersistedUser(user)
  ) {
    return null;
  }
  return { accessToken, refreshToken, expiresAtUnixMilliseconds, user };
}

function isPersistedUser(value: unknown): value is PersistedAuthSession["user"] {
  return (
    isRecord(value) &&
    isNonEmptyString(value.id) &&
    typeof value.email === "string" &&
    typeof value.status === "string" &&
    typeof value.authorizationEpoch === "number"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}
