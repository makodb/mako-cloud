import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import {
  isApiErrorEnvelope,
  type ApiErrorEnvelope,
  type MakoAuthSession,
  type MakoAuthUser,
  type MakoEmailVerified,
  type MakoMagicLinkAccepted,
  type MakoProviderSignInStart,
  type MakoSignUpAccepted,
} from "./wire.js";

export type {
  MakoAuthSession,
  MakoAuthUser,
  MakoEmailVerified,
  MakoMagicLinkAccepted,
  MakoProviderSignInStart,
  MakoSignUpAccepted,
};

/**
 * Shorter aliases for the payload types above, kept because they are the names
 * this package published first. `MakoAuthUser`, `MakoSignUpAccepted`, and
 * `MakoProviderSignInStart` are the preferred spellings in new code.
 */
export type AuthUser = MakoAuthUser;
export type SignUpAccepted = MakoSignUpAccepted;
export type ProviderSignInStart = MakoProviderSignInStart;

export interface PersistedAuthSession {
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly expiresAtUnixMilliseconds: number;
  readonly user: MakoAuthUser;
}

export interface MakoUserSession {
  readonly accessToken: string;
  readonly expiresAtUnixMilliseconds: number;
  readonly user: MakoAuthUser;
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

/**
 * What a location fragment carries when the browser lands on a registered
 * redirect: a provider's one-time code, a magic link's token, a provider's
 * refusal, or nothing sign-in related.
 */
export type MakoSignInFragment =
  | { readonly kind: "provider_code"; readonly value: string }
  | { readonly kind: "magic_link"; readonly value: string }
  | { readonly kind: "error"; readonly value: string }
  | { readonly kind: "none"; readonly value: null };

/**
 * What an application observes without polling: a session was issued, the
 * client signed out, a refresh could not reach the service (the session was
 * kept), or a later refresh succeeded and ended that outage.
 */
export type MakoAuthEventKind =
  | "session"
  | "signed_out"
  | "refresh_unavailable"
  | "refresh_recovered";

export interface MakoAuthEvent {
  readonly kind: MakoAuthEventKind;
  /** The session in force when the event was emitted, or `null` once signed out. */
  readonly session: MakoUserSession | null;
}

export type MakoAuthEventListener = (event: MakoAuthEvent) => void;

/** Stops a subscription; calling it more than once is harmless. */
export type MakoAuthUnsubscribe = () => void;

export interface MakoAuthErrorOptions {
  readonly apiError?: ApiErrorEnvelope;
  readonly status?: number;
  /** The stable reason a provider callback reported in its `#error=` fragment. */
  readonly reason?: string;
  /** A stable client-side classification; defaults to the API error code. */
  readonly code?: string;
  /** Whether the same operation may be retried later without signing the user out. */
  readonly retryable?: boolean;
}

export class MakoAuthError extends Error {
  override readonly name: string = "MakoAuthError";
  readonly apiError?: ApiErrorEnvelope;
  readonly status?: number;
  readonly reason?: string;
  /** `refresh_unavailable` for a transient refresh failure, else the API error code. */
  readonly code: string;
  /** A transient failure: the stored session was kept and the call may be retried. */
  readonly retryable: boolean;

  constructor(message: string, options: MakoAuthErrorOptions = {}) {
    super(message);
    if (options.apiError !== undefined) {
      this.apiError = options.apiError;
    }
    if (options.status !== undefined) {
      this.status = options.status;
    }
    if (options.reason !== undefined) {
      this.reason = options.reason;
    }
    this.code = options.code ?? options.apiError?.error.code ?? "authentication_failed";
    this.retryable = options.retryable ?? false;
  }
}

export class MakoAuthenticationRequiredError extends MakoAuthError {
  override readonly name: string = "MakoAuthenticationRequiredError";
}

const PROVIDER_NAME = /^[a-z0-9-]{2,64}$/u;
const MAXIMUM_REDIRECT_URL_LENGTH = 2_048;

export class MakoAuthClient {
  readonly #config: NormalizedMakoRxdbClientConfig;
  readonly #persistence: AuthSessionPersistence;
  readonly #fetch: typeof globalThis.fetch;
  readonly #now: () => number;
  readonly #listeners = new Set<MakoAuthEventListener>();
  #session: PersistedAuthSession | null = null;
  #authenticationRequired = false;
  #refreshUnavailable = false;
  #refreshInFlight: Promise<MakoUserSession> | null = null;

  constructor(config: NormalizedMakoRxdbClientConfig, options: MakoAuthClientOptions = {}) {
    this.#config = config;
    this.#persistence = options.persistence ?? new MemoryAuthSessionPersistence();
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#now = options.now ?? Date.now;
  }

  /**
   * Classify a location fragment (`location.hash`, with or without the
   * leading `#`) so an application can decide on load whether it is finishing
   * a provider sign-in, redeeming a magic link, showing a provider's refusal,
   * or doing nothing. Values are percent-decoded; nothing is validated
   * against the service here.
   */
  static signInFragment(fragment: string): MakoSignInFragment {
    const parameters = new URLSearchParams(fragment.startsWith("#") ? fragment.slice(1) : fragment);
    const error = parameters.get("error");
    if (error !== null && error.length > 0) {
      return { kind: "error", value: error };
    }
    const code = parameters.get("code");
    if (code !== null && code.length > 0) {
      return { kind: "provider_code", value: code };
    }
    const token = parameters.get("magic_link_token");
    if (token !== null && token.length > 0) {
      return { kind: "magic_link", value: token };
    }
    return { kind: "none", value: null };
  }

  /**
   * The token a verification link opened the page with, from its
   * `#verification_token=` fragment, or `null`. Kept apart from
   * `signInFragment` because redeeming it confirms an address and issues no
   * session.
   */
  static verificationFragment(fragment: string): string | null {
    const parameters = new URLSearchParams(fragment.startsWith("#") ? fragment.slice(1) : fragment);
    const token = parameters.get("verification_token");
    return token !== null && token.length > 0 ? token : null;
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

  /**
   * True while a refresh has failed for a reason that says nothing about the
   * stored credential -- no network, a timeout, a `5xx`, or a `429`. The
   * session is kept, local data stays readable, and the next successful
   * refresh clears this.
   */
  get refreshUnavailable(): boolean {
    return this.#refreshUnavailable;
  }

  /**
   * Observe session transitions without polling, so an application can show
   * "offline -- working from local data" and take it down again. Returns the
   * function that stops the subscription. A listener that throws is ignored:
   * one subscriber cannot break another or the session it observed.
   */
  subscribe(listener: MakoAuthEventListener): MakoAuthUnsubscribe {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  async validAccessToken(minimumValidityMilliseconds = 30_000): Promise<string> {
    const session = this.#session ?? (await this.#persistence.load());
    if (session === null) {
      this.#authenticationRequired = true;
      throw new MakoAuthenticationRequiredError("an application-user session is required", {
        code: "unauthenticated",
      });
    }
    this.#session = session;
    if (session.expiresAtUnixMilliseconds > this.#now() + minimumValidityMilliseconds) {
      return session.accessToken;
    }
    try {
      return (await this.refreshSession()).accessToken;
    } catch (error) {
      // A refresh that never reached a verdict must not sign anyone out: keep
      // using the token while it is still valid, and report a retryable
      // failure once it is not.
      const current = this.#session;
      if (
        error instanceof MakoAuthError &&
        error.retryable &&
        current !== null &&
        current.expiresAtUnixMilliseconds > this.#now()
      ) {
        return current.accessToken;
      }
      throw error;
    }
  }

  /**
   * Register an address. Where the environment verifies email addresses,
   * `redirectUrl` must be one of its registered redirects: the mailed link
   * lands there with `#verification_token=…`, for `verifyEmail`.
   */
  async signUp(
    email: string,
    password: string,
    options: { readonly redirectUrl?: string } = {},
  ): Promise<MakoSignUpAccepted> {
    if (options.redirectUrl !== undefined) {
      assertRedirectUrl(options.redirectUrl);
    }
    return this.#request<MakoSignUpAccepted>("signup", {
      method: "POST",
      body: JSON.stringify({
        email,
        password,
        ...(options.redirectUrl === undefined ? {} : { redirectUrl: options.redirectUrl }),
      }),
    });
  }

  /**
   * Redeem the token a verification link carried in its
   * `#verification_token=` fragment. The account can then sign in; no
   * session is issued, so the password is still asked for.
   */
  async verifyEmail(token: string): Promise<MakoEmailVerified> {
    if (token.length < 1 || token.length > 256) {
      throw new MakoAuthError("verification token is invalid");
    }
    return this.#request<MakoEmailVerified>(
      "verify-email",
      { method: "POST", body: JSON.stringify({ token }) },
      200,
    );
  }

  async signInWithPassword(email: string, password: string): Promise<MakoUserSession> {
    const wire = await this.#request<MakoAuthSession>("signin", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
    return this.#persistWireSession(wire);
  }

  /**
   * Ask the service where to send the browser for an enabled external
   * provider. `redirectUrl` must be one of the environment's registered
   * redirects; the browser comes back there with `#code=…` or `#error=…`.
   */
  async startProviderSignIn(
    provider: string,
    redirectUrl: string,
  ): Promise<MakoProviderSignInStart> {
    if (!PROVIDER_NAME.test(provider)) {
      throw new MakoAuthError("provider name is invalid");
    }
    assertRedirectUrl(redirectUrl);
    return this.#request<MakoProviderSignInStart>(
      `providers/${encodeURIComponent(provider)}/start`,
      { method: "POST", body: JSON.stringify({ redirectUrl }) },
      200,
    );
  }

  /**
   * Finish a provider sign-in from the fragment the browser landed with.
   * A `#error=` fragment throws `MakoAuthError` carrying the provider's
   * reason; a `#code=` fragment is exchanged for a session that is stored
   * in the configured persistence and behaves like a password session.
   */
  async completeProviderSignIn(fragment: string): Promise<MakoUserSession> {
    const parsed = MakoAuthClient.signInFragment(fragment);
    if (parsed.kind === "error") {
      throw new MakoAuthError(`provider sign-in was refused: ${parsed.value}`, {
        reason: parsed.value,
      });
    }
    if (parsed.kind !== "provider_code") {
      throw new MakoAuthError("the location fragment carries no provider sign-in code");
    }
    const wire = await this.#request<MakoAuthSession>(
      "providers/exchange",
      { method: "POST", body: JSON.stringify({ code: parsed.value }) },
      200,
    );
    return this.#persistWireSession(wire);
  }

  /**
   * Ask for a single-use sign-in link by email. The service accepts any
   * well-formed address without revealing whether it is registered, so a
   * resolved promise means only that the request was accepted.
   */
  async requestMagicLink(email: string, redirectUrl: string): Promise<void> {
    assertRedirectUrl(redirectUrl);
    await this.#request<MakoMagicLinkAccepted>(
      "magic-link",
      { method: "POST", body: JSON.stringify({ email, redirectUrl }) },
      202,
    );
  }

  /** Redeem the token a magic link carried in its `#magic_link_token=` fragment. */
  async redeemMagicLink(token: string): Promise<MakoUserSession> {
    if (token.length < 1 || token.length > 2_048) {
      throw new MakoAuthError("magic link token is invalid");
    }
    const wire = await this.#request<MakoAuthSession>(
      "magic-link/redeem",
      { method: "POST", body: JSON.stringify({ token }) },
      200,
    );
    return this.#persistWireSession(wire);
  }

  /**
   * Rotate the session. One request is in flight at a time: concurrent
   * callers -- `refreshSession()` and `validAccessToken()` alike -- await the
   * same promise and receive the same session, so two replication scopes
   * reacting to one authorization-epoch signal cannot spend the rotated
   * credential twice and have the second spend read as a replay. The next
   * call after it settles starts a new request.
   */
  async refreshSession(): Promise<MakoUserSession> {
    const inFlight = this.#refreshInFlight;
    if (inFlight !== null) {
      return await inFlight;
    }
    const attempt = this.#rotateSession();
    this.#refreshInFlight = attempt;
    try {
      return await attempt;
    } finally {
      if (this.#refreshInFlight === attempt) {
        this.#refreshInFlight = null;
      }
    }
  }

  async #rotateSession(): Promise<MakoUserSession> {
    const session = this.#session ?? (await this.#persistence.load());
    if (session === null) {
      this.#authenticationRequired = true;
      throw new MakoAuthenticationRequiredError("no refresh session is available", {
        code: "unauthenticated",
      });
    }
    this.#session = session;
    try {
      const wire = await this.#request<MakoAuthSession>("token", {
        method: "POST",
        body: JSON.stringify({ refreshToken: session.refreshToken }),
      });
      return await this.#persistWireSession(wire);
    } catch (error) {
      if (!isDefinitiveRefusal(error)) {
        throw this.#refreshUnavailableError(error);
      }
      await this.#requireAuthentication();
      throw new MakoAuthenticationRequiredError(
        "the application-user session cannot be refreshed",
        { ...carriedErrorOptions(error), code: "unauthenticated" },
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
      await this.#requireAuthentication();
    }
  }

  async #persistWireSession(wire: MakoAuthSession): Promise<MakoUserSession> {
    const session: PersistedAuthSession = {
      accessToken: wire.accessToken,
      refreshToken: wire.refreshToken,
      expiresAtUnixMilliseconds: this.#now() + wire.expiresIn * 1_000,
      user: wire.user,
    };
    await this.#persistence.save(session);
    this.#session = session;
    this.#authenticationRequired = false;
    const recovered = this.#refreshUnavailable;
    this.#refreshUnavailable = false;
    this.#emit("session");
    if (recovered) {
      this.#emit("refresh_recovered");
    }
    return publicSession(session) as MakoUserSession;
  }

  async #requireAuthentication(): Promise<void> {
    this.#session = null;
    this.#authenticationRequired = true;
    this.#refreshUnavailable = false;
    await this.#persistence.clear();
    this.#emit("signed_out");
  }

  /** Keep the stored session, report the outage once, and stay retryable. */
  #refreshUnavailableError(error: unknown): MakoAuthError {
    if (!this.#refreshUnavailable) {
      this.#refreshUnavailable = true;
      this.#emit("refresh_unavailable");
    }
    return new MakoAuthError(
      "the application-user session could not be refreshed; the stored session was kept",
      { ...carriedErrorOptions(error), code: "refresh_unavailable", retryable: true },
    );
  }

  #emit(kind: MakoAuthEventKind): void {
    const event: MakoAuthEvent = { kind, session: publicSession(this.#session) };
    for (const listener of [...this.#listeners]) {
      try {
        listener(event);
      } catch {
        // A subscriber's failure must not change the session it observed.
      }
    }
  }

  async #request<T>(route: string, init: RequestInit, expectedStatus?: number): Promise<T> {
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
      throw new MakoAuthError("authentication service is unavailable", {
        code: "unavailable",
        retryable: true,
      });
    }
    if (!response.ok || (expectedStatus !== undefined && response.status !== expectedStatus)) {
      const body: unknown = await response.json().catch(() => null);
      const retryable = isTransientStatus(response.status);
      if (isApiErrorEnvelope(body)) {
        throw new MakoAuthError(body.error.message, {
          apiError: body,
          status: response.status,
          retryable,
        });
      }
      throw new MakoAuthError("authentication request failed", {
        status: response.status,
        retryable,
      });
    }
    if (response.status === 204) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }
}

/**
 * Whether the service definitively refused the stored refresh credential --
 * the only outcome that may sign a user out. The token route answers `401
 * unauthenticated` for an invalid or replayed credential and `400
 * invalid_request` for one it cannot parse, while a `429`, a `5xx`, or no
 * response at all says nothing about the credential.
 */
function isDefinitiveRefusal(error: unknown): boolean {
  if (!(error instanceof MakoAuthError) || error.status === undefined) {
    return false;
  }
  if (isTransientStatus(error.status)) {
    return false;
  }
  const code = error.apiError?.error.code;
  if (code === "unauthenticated" || code === "permission_denied") {
    return true;
  }
  return error.status >= 400 && error.status < 500;
}

function isTransientStatus(status: number): boolean {
  return status === 408 || status === 429 || status >= 500;
}

function carriedErrorOptions(error: unknown): MakoAuthErrorOptions {
  const options: { apiError?: ApiErrorEnvelope; status?: number } = {};
  if (error instanceof MakoAuthError) {
    if (error.apiError !== undefined) {
      options.apiError = error.apiError;
    }
    if (error.status !== undefined) {
      options.status = error.status;
    }
  }
  return options;
}

function assertRedirectUrl(redirectUrl: string): void {
  let parsed: URL;
  try {
    parsed = new URL(redirectUrl);
  } catch {
    throw new MakoAuthError("redirectUrl must be an absolute URL");
  }
  if (
    redirectUrl.length > MAXIMUM_REDIRECT_URL_LENGTH ||
    parsed.hash !== "" ||
    parsed.username !== "" ||
    parsed.password !== ""
  ) {
    throw new MakoAuthError("redirectUrl must carry no fragment or credentials");
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
