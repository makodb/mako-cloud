import { isApiErrorEnvelope, type ApiErrorEnvelope, type components } from "@mako-cloud/api-types";

import type { NormalizedMakoRxdbClientConfig } from "./config.js";

export type AuthUser = components["schemas"]["AuthUser"];
export type SignUpAccepted = components["schemas"]["SignUpAccepted"];
type WireAuthSession = components["schemas"]["AuthSession"];

export interface PersistedAuthSession {
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAtUnixMilliseconds: number;
  readonly user: AuthUser;
}

export interface MakoUserSession {
  readonly accessToken: string;
  readonly expiresAtUnixMilliseconds: number;
  readonly user: AuthUser;
}

export interface AuthSessionPersistence {
  load(): Promise<PersistedAuthSession | null>;
  save(session: PersistedAuthSession): Promise<void>;
  clear(): Promise<void>;
}

export class MemoryAuthSessionPersistence implements AuthSessionPersistence {
  #session: PersistedAuthSession | null = null;

  async load(): Promise<PersistedAuthSession | null> {
    return this.#session === null ? null : structuredClone(this.#session);
  }

  async save(session: PersistedAuthSession): Promise<void> {
    this.#session = structuredClone(session);
  }

  async clear(): Promise<void> {
    this.#session = null;
  }
}

export interface MakoAuthClientOptions {
  readonly persistence?: AuthSessionPersistence;
  readonly fetch?: typeof globalThis.fetch;
  readonly now?: () => number;
}

export class MakoAuthError extends Error {
  override readonly name: string = "MakoAuthError";
  readonly apiError?: ApiErrorEnvelope;
  readonly status?: number;

  constructor(message: string, options: { apiError?: ApiErrorEnvelope; status?: number } = {}) {
    super(message);
    if (options.apiError !== undefined) {
      this.apiError = options.apiError;
    }
    if (options.status !== undefined) {
      this.status = options.status;
    }
  }
}

export class MakoAuthenticationRequiredError extends MakoAuthError {
  override readonly name: string = "MakoAuthenticationRequiredError";
}

export class MakoAuthClient {
  readonly #config: NormalizedMakoRxdbClientConfig;
  readonly #persistence: AuthSessionPersistence;
  readonly #fetch: typeof globalThis.fetch;
  readonly #now: () => number;
  #session: PersistedAuthSession | null = null;
  #authenticationRequired = false;
  #refreshInFlight: Promise<MakoUserSession> | null = null;

  constructor(config: NormalizedMakoRxdbClientConfig, options: MakoAuthClientOptions = {}) {
    this.#config = config;
    this.#persistence = options.persistence ?? new MemoryAuthSessionPersistence();
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#now = options.now ?? Date.now;
  }

  async restoreSession(): Promise<MakoUserSession | null> {
    this.#session = await this.#persistence.load();
    this.#authenticationRequired = this.#session === null;
    return publicSession(this.#session);
  }

  currentSession(): MakoUserSession | null {
    return publicSession(this.#session);
  }

  accessToken(): string | null {
    return this.#session?.accessToken ?? null;
  }

  get authenticationRequired(): boolean {
    return this.#authenticationRequired;
  }

  async validAccessToken(minimumValidityMilliseconds = 30_000): Promise<string> {
    const session = this.#session ?? (await this.#persistence.load());
    if (session === null) {
      this.#authenticationRequired = true;
      throw new MakoAuthenticationRequiredError("an application-user session is required");
    }
    this.#session = session;
    if (session.expiresAtUnixMilliseconds > this.#now() + minimumValidityMilliseconds) {
      return session.accessToken;
    }
    this.#refreshInFlight ??= this.refreshSession().finally(() => {
      this.#refreshInFlight = null;
    });
    return (await this.#refreshInFlight).accessToken;
  }

  async signUp(email: string, password: string): Promise<SignUpAccepted> {
    return this.#request<SignUpAccepted>("signup", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
  }

  async signInWithPassword(email: string, password: string): Promise<MakoUserSession> {
    const wire = await this.#request<WireAuthSession>("signin", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
    return this.#persistWireSession(wire);
  }

  async refreshSession(): Promise<MakoUserSession> {
    const session = this.#session ?? (await this.#persistence.load());
    if (session === null) {
      this.#authenticationRequired = true;
      throw new MakoAuthenticationRequiredError("no refresh session is available");
    }
    try {
      const wire = await this.#request<WireAuthSession>("token", {
        method: "POST",
        body: JSON.stringify({ refreshToken: session.refreshToken }),
      });
      return this.#persistWireSession(wire);
    } catch (error) {
      await this.#requireAuthentication();
      const options: { apiError?: ApiErrorEnvelope; status?: number } = {};
      if (error instanceof MakoAuthError && error.apiError !== undefined) {
        options.apiError = error.apiError;
      }
      if (error instanceof MakoAuthError && error.status !== undefined) {
        options.status = error.status;
      }
      throw new MakoAuthenticationRequiredError(
        "the application-user session cannot be refreshed",
        options,
      );
    }
  }

  async signOut(): Promise<void> {
    const session = this.#session ?? (await this.#persistence.load());
    try {
      if (session !== null) {
        await this.#request<void>("signout", {
          method: "POST",
          headers: { Authorization: `Bearer ${session.accessToken}` },
        });
      }
    } finally {
      this.#session = null;
      this.#authenticationRequired = true;
      await this.#persistence.clear();
    }
  }

  async #persistWireSession(wire: WireAuthSession): Promise<MakoUserSession> {
    const session: PersistedAuthSession = {
      accessToken: wire.accessToken,
      refreshToken: wire.refreshToken,
      expiresAtUnixMilliseconds: this.#now() + wire.expiresIn * 1_000,
      user: wire.user,
    };
    await this.#persistence.save(session);
    this.#session = session;
    this.#authenticationRequired = false;
    return publicSession(session) as MakoUserSession;
  }

  async #requireAuthentication(): Promise<void> {
    this.#session = null;
    this.#authenticationRequired = true;
    await this.#persistence.clear();
  }

  async #request<T>(route: string, init: RequestInit): Promise<T> {
    const endpoint = new URL(
      `v1/projects/${encodeURIComponent(this.#config.projectId)}/environments/${encodeURIComponent(
        this.#config.environmentId,
      )}/auth/${route}`,
      withTrailingSlash(this.#config.endpoint),
    );
    let response: Response;
    try {
      response = await this.#fetch(endpoint, {
        ...init,
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
          "X-Mako-Key": this.#config.publicProjectKey,
          ...init.headers,
        },
      });
    } catch {
      throw new MakoAuthError("authentication service is unavailable");
    }
    if (!response.ok) {
      const body: unknown = await response.json().catch(() => null);
      if (isApiErrorEnvelope(body)) {
        throw new MakoAuthError(body.error.message, { apiError: body, status: response.status });
      }
      throw new MakoAuthError("authentication request failed", { status: response.status });
    }
    if (response.status === 204) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }
}

function publicSession(session: PersistedAuthSession | null): MakoUserSession | null {
  if (session === null) {
    return null;
  }
  return {
    accessToken: session.accessToken,
    expiresAtUnixMilliseconds: session.expiresAtUnixMilliseconds,
    user: session.user,
  };
}

function withTrailingSlash(url: URL): URL {
  const normalized = new URL(url);
  normalized.pathname = `${normalized.pathname.replace(/\/+$/u, "")}/`;
  return normalized;
}
