import {
  createDeveloperAuthClient,
  type DeveloperSession as DeveloperSessionResponse,
  type MakoDeveloperAuthClient,
} from "@mako-cloud/management-sdk";

import type {
  DeveloperAuthAdapter,
  DeveloperProfile,
  DeveloperSelfServiceAdapter,
  DeveloperSession,
} from "./auth.js";

const SESSION_STORAGE_KEY = "mako.console.developer-session.v1";
const MANAGEMENT_AUDIENCE = "mako-management";
const MAXIMUM_SESSION_SECONDS = 3_600;
const MAXIMUM_TOKEN_BYTES = 16 * 1_024;

interface SessionStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export interface ShortLivedDeveloperSessionAuthOptions {
  readonly managementEndpoint: string;
  readonly storage: SessionStorage;
  readonly now?: (() => number) | undefined;
}

export interface SessionTokenDeveloperAuthAdapter extends DeveloperAuthAdapter {
  acceptSessionToken(token: string): Promise<void>;
}

export interface HostedDeveloperAuthOptions extends ShortLivedDeveloperSessionAuthOptions {}

/** Same-origin hosted identity adapter backed by the public developer-auth API. */
export class HostedDeveloperAuthAdapter implements DeveloperSelfServiceAdapter {
  readonly #client: MakoDeveloperAuthClient;
  readonly #issuer: string;
  readonly #storage: SessionStorage;
  readonly #now: () => number;
  readonly #listeners = new Set<(session: DeveloperSession | null) => void>();

  constructor(options: HostedDeveloperAuthOptions) {
    const endpoint = normalizedManagementEndpoint(options.managementEndpoint);
    this.#client = createDeveloperAuthClient({ endpoint });
    this.#issuer = `${endpoint}/control-identity`;
    this.#storage = options.storage;
    this.#now = options.now ?? Date.now;
  }

  async loadSession(): Promise<DeveloperSession | null> {
    const stored = this.#storedToken();
    if (stored !== null) {
      try {
        return parseHostedDeveloperSession(stored, this.#issuer, this.#now());
      } catch {
        this.#removeStoredSession();
      }
    }
    try {
      return this.#acceptResponse(await this.#client.refresh());
    } catch {
      return null;
    }
  }

  async beginSignIn(_returnTo: string): Promise<void> {
    throw authenticationError("Enter your developer email and password to continue.");
  }

  async register(input: {
    readonly email: string;
    readonly displayName: string;
    readonly password: string;
  }): Promise<void> {
    await this.#client.register(input);
  }

  async verifyEmail(token: string): Promise<"waitlisted" | "password_updated"> {
    return (await this.#client.verifyEmail(validTokenInput(token))).status;
  }

  async resendVerification(email: string): Promise<void> {
    await this.#client.resendVerification(email);
  }

  async signInWithPassword(email: string, password: string): Promise<DeveloperSession> {
    const session = this.#acceptResponse(await this.#client.signIn(email, password));
    this.#emit(session);
    return session;
  }

  async requestPasswordRecovery(email: string): Promise<void> {
    await this.#client.requestPasswordRecovery(email);
  }

  async completePasswordRecovery(
    token: string,
    password: string,
  ): Promise<"waitlisted" | "password_updated"> {
    return (await this.#client.completePasswordRecovery(validTokenInput(token), password)).status;
  }

  async waitListStatus(): Promise<{
    readonly developerIdentityId: string;
    readonly status: "waitlisted";
  }> {
    const session = await this.loadSession();
    if (session?.audience !== "mako-developer-waitlist") {
      throw authenticationError("Your wait-list session expired. Sign in again to continue.");
    }
    return this.#client.waitListStatus(session.accessToken);
  }

  async signOut(): Promise<void> {
    try {
      await this.#client.signOut();
    } finally {
      this.#removeStoredSession();
      this.#emit(null);
    }
  }

  subscribe(listener: (session: DeveloperSession | null) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #acceptResponse(response: DeveloperSessionResponse): DeveloperSession {
    const session = parseHostedDeveloperSession(response.accessToken, this.#issuer, this.#now());
    const responseExpiresAt = Date.parse(response.expiresAt);
    if (
      session.audience !== response.audience ||
      (response.status === "active") !== (session.audience === MANAGEMENT_AUDIENCE) ||
      !Number.isFinite(responseExpiresAt) ||
      Date.parse(session.expiresAt) !== responseExpiresAt
    ) {
      throw authenticationError("The developer session response is invalid.");
    }
    try {
      this.#storage.setItem(SESSION_STORAGE_KEY, response.accessToken);
    } catch {
      throw authenticationError("Browser session storage is unavailable.");
    }
    return session;
  }

  #storedToken(): string | null {
    try {
      return this.#storage.getItem(SESSION_STORAGE_KEY);
    } catch {
      throw authenticationError("Browser session storage is unavailable.");
    }
  }

  #removeStoredSession() {
    try {
      this.#storage.removeItem(SESSION_STORAGE_KEY);
    } catch {
      // Treat an unavailable tab store as signed out.
    }
  }

  #emit(session: DeveloperSession | null) {
    for (const listener of this.#listeners) listener(session);
  }
}

/**
 * Hosted-preview adapter for the control plane's short-lived, signed developer
 * sessions. The credential is scoped to one browser tab and is never placed in
 * a URL, cookie, local storage, log, or release artifact. The client-side claim
 * checks only shape the UI; every management request is authenticated again by
 * the control plane's signature verifier.
 */
export class ShortLivedDeveloperSessionAuthAdapter implements SessionTokenDeveloperAuthAdapter {
  readonly #issuer: string;
  readonly #storage: SessionStorage;
  readonly #now: () => number;
  readonly #listeners = new Set<(session: DeveloperSession | null) => void>();

  constructor(options: ShortLivedDeveloperSessionAuthOptions) {
    const endpoint = normalizedManagementEndpoint(options.managementEndpoint);
    this.#issuer = `${endpoint}/control-identity`;
    this.#storage = options.storage;
    this.#now = options.now ?? Date.now;
  }

  async loadSession(): Promise<DeveloperSession | null> {
    let token: string | null;
    try {
      token = this.#storage.getItem(SESSION_STORAGE_KEY);
    } catch {
      throw authenticationError("Browser session storage is unavailable.");
    }
    if (token === null) return null;
    try {
      return parseShortLivedDeveloperSession(token, this.#issuer, this.#now());
    } catch {
      this.#removeStoredSession();
      return null;
    }
  }

  async beginSignIn(_returnTo: string): Promise<void> {
    throw authenticationError("Enter a short-lived developer session token to continue.");
  }

  async acceptSessionToken(token: string): Promise<void> {
    const normalized = token.trim();
    const session = parseShortLivedDeveloperSession(normalized, this.#issuer, this.#now());
    try {
      this.#storage.setItem(SESSION_STORAGE_KEY, normalized);
    } catch {
      throw authenticationError("Browser session storage is unavailable.");
    }
    this.#emit(session);
  }

  async signOut(): Promise<void> {
    this.#removeStoredSession();
    this.#emit(null);
  }

  subscribe(listener: (session: DeveloperSession | null) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #removeStoredSession() {
    try {
      this.#storage.removeItem(SESSION_STORAGE_KEY);
    } catch {
      // An unavailable storage backend is already treated as signed out.
    }
  }

  #emit(session: DeveloperSession | null) {
    for (const listener of this.#listeners) listener(session);
  }
}

export function isSessionTokenDeveloperAuthAdapter(
  adapter: DeveloperAuthAdapter,
): adapter is SessionTokenDeveloperAuthAdapter {
  return "acceptSessionToken" in adapter && typeof adapter.acceptSessionToken === "function";
}

export function parseShortLivedDeveloperSession(
  token: string,
  expectedIssuer: string,
  now = Date.now(),
): DeveloperSession {
  const session = parseHostedDeveloperSession(token, expectedIssuer, now);
  if (session.audience !== MANAGEMENT_AUDIENCE) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  return session;
}

export function parseHostedDeveloperSession(
  token: string,
  expectedIssuer: string,
  now = Date.now(),
): DeveloperSession {
  if (
    token.length < 64 ||
    token.length > MAXIMUM_TOKEN_BYTES ||
    token.trim() !== token ||
    token.includes("\n") ||
    token.includes("\r")
  ) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  const segments = token.split(".");
  if (segments.length !== 3 || segments.some((segment) => segment.length === 0)) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  const header = jsonObject(decodeBase64Url(segments[0] ?? ""));
  const claims = jsonObject(decodeBase64Url(segments[1] ?? ""));
  if (
    header.alg !== "EdDSA" ||
    header.typ !== "JWT" ||
    typeof header.kid !== "string" ||
    !/^devkid_[a-f0-9]{16}$/u.test(header.kid)
  ) {
    throw authenticationError("The developer session token is invalid or expired.");
  }

  const identityId = boundedString(claims.developerIdentityId, 12, 68);
  const email = boundedString(claims.email, 3, 320);
  const displayName = boundedString(claims.name, 1, 200);
  const issuer = boundedString(claims.iss, 8, 2_048);
  const subject = boundedString(claims.sub, 3, 320);
  const issuedAt = safeInteger(claims.iat);
  const expiresAt = safeInteger(claims.exp);
  const authorizationEpoch = safeInteger(claims.authorizationEpoch);
  const audiences = claims.aud;
  const status = claims.status;
  const audience = Array.isArray(audiences) && audiences.length === 1 ? audiences[0] : null;
  if (
    !/^dev_[A-Za-z0-9_-]{8,64}$/u.test(identityId) ||
    issuer !== expectedIssuer ||
    (subject !== identityId && subject !== email) ||
    claims.emailVerified !== true ||
    authorizationEpoch === 0 ||
    (status !== "active" && status !== "waitlisted") ||
    (audience !== MANAGEMENT_AUDIENCE && audience !== "mako-developer-waitlist") ||
    (status === "active") !== (audience === MANAGEMENT_AUDIENCE) ||
    issuedAt > Math.floor(now / 1_000) + 60 ||
    expiresAt <= Math.floor(now / 1_000) ||
    expiresAt <= issuedAt ||
    expiresAt - issuedAt > MAXIMUM_SESSION_SECONDS
  ) {
    throw authenticationError("The developer session token is invalid or expired.");
  }

  const profile: DeveloperProfile = { id: identityId, email, displayName };
  return {
    accessToken: token,
    expiresAt: new Date(expiresAt * 1_000).toISOString(),
    audience,
    profile,
  };
}

function validTokenInput(token: string): string {
  const normalized = token.trim();
  if (
    normalized.length < 32 ||
    normalized.length > 4_096 ||
    normalized !== token ||
    normalized.includes("\n") ||
    normalized.includes("\r")
  ) {
    throw authenticationError("The single-use link is invalid or expired.");
  }
  return normalized;
}

function normalizedManagementEndpoint(value: string): string {
  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch {
    throw authenticationError("The management endpoint is invalid.");
  }
  if (
    endpoint.username !== "" ||
    endpoint.password !== "" ||
    endpoint.search !== "" ||
    endpoint.hash !== "" ||
    endpoint.pathname !== "/" ||
    (endpoint.protocol !== "https:" &&
      !(endpoint.protocol === "http:" && ["127.0.0.1", "localhost"].includes(endpoint.hostname)))
  ) {
    throw authenticationError("The management endpoint is invalid.");
  }
  return endpoint.origin;
}

function decodeBase64Url(value: string): string {
  if (!/^[A-Za-z0-9_-]+$/u.test(value)) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  const padded = `${value.replaceAll("-", "+").replaceAll("_", "/")}${"=".repeat(
    (4 - (value.length % 4)) % 4,
  )}`;
  try {
    const binary = globalThis.atob(padded);
    return new TextDecoder("utf-8", { fatal: true }).decode(
      Uint8Array.from(binary, (character) => character.charCodeAt(0)),
    );
  } catch {
    throw authenticationError("The developer session token is invalid or expired.");
  }
}

function jsonObject(value: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  return parsed as Record<string, unknown>;
}

function boundedString(value: unknown, minimum: number, maximum: number): string {
  if (
    typeof value !== "string" ||
    value.length < minimum ||
    value.length > maximum ||
    value.trim() !== value ||
    [...value].some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint < 32 || codePoint === 127;
    })
  ) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  return value;
}

function safeInteger(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw authenticationError("The developer session token is invalid or expired.");
  }
  return value;
}

function authenticationError(message: string): Error {
  const error = new Error(message);
  error.name = "DeveloperAuthenticationError";
  return error;
}
