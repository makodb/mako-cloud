import {
  createOperatorClient,
  ManagementApiError,
  type MakoOperatorClient,
  type OperatorSession as ApiOperatorSession,
} from "@mako-cloud/management-sdk";

import type { OperatorAuthAdapter, OperatorSession } from "./operator-auth.js";

const ALLOWED_PERMISSIONS = new Set([
  "tenant_read",
  "overview_read",
  "operations_read",
  "incident_read",
  "incident_manage",
  "backup_read",
  "recovery_manage",
  "fleet_read",
  "security_read",
  "security_manage",
  "activity_read",
  "activity_export",
  "provisioning_repair",
  "quota_override",
  "abuse_response",
  "support_access",
  "waitlist_review",
]);
const ALLOWED_DEVELOPER_STATUSES = new Set([
  "unverified",
  "waitlisted",
  "active",
  "rejected",
  "disabled",
  "deleted",
]);

export interface HostedOperatorAuthOptions {
  readonly managementEndpoint: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly now?: (() => number) | undefined;
}

/** Same-origin operator adapter backed only by a protected HttpOnly cookie. */
export class HostedOperatorAuthAdapter implements OperatorAuthAdapter {
  readonly #client: MakoOperatorClient;
  readonly #now: () => number;
  readonly #listeners = new Set<(session: OperatorSession | null) => void>();

  constructor(options: HostedOperatorAuthOptions) {
    this.#client = createOperatorClient({
      endpoint: normalizedEndpoint(options.managementEndpoint),
      ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
    });
    this.#now = options.now ?? Date.now;
  }

  async loadSession(): Promise<OperatorSession | null> {
    try {
      return parseHostedOperatorSession(await this.#client.currentSession(), this.#now());
    } catch (error) {
      if (error instanceof ManagementApiError && error.status === 401) return null;
      throw safeOperatorAuthenticationError(error);
    }
  }

  async signIn(email: string, password: string): Promise<OperatorSession> {
    try {
      const session = parseHostedOperatorSession(
        await this.#client.signIn(email, password),
        this.#now(),
      );
      this.#emit(session);
      return session;
    } catch (error) {
      throw safeOperatorAuthenticationError(error);
    }
  }

  async verifyPassword(password: string): Promise<OperatorSession> {
    try {
      const session = parseHostedOperatorSession(
        await this.#client.verifyPassword(password),
        this.#now(),
      );
      this.#emit(session);
      return session;
    } catch (error) {
      throw safeOperatorAuthenticationError(error);
    }
  }

  async signOut(): Promise<void> {
    try {
      await this.#client.signOut();
    } finally {
      this.#emit(null);
    }
  }

  subscribe(listener: (session: OperatorSession | null) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #emit(session: OperatorSession | null) {
    for (const listener of this.#listeners) listener(session);
  }
}

export function parseHostedOperatorSession(
  value: ApiOperatorSession,
  now = Date.now(),
): OperatorSession {
  const expiresAt = Date.parse(value.expiresAt);
  const passwordVerifiedAt = Date.parse(value.passwordVerifiedAt);
  if (
    !/^opr_[A-Za-z0-9_-]{8,64}$/u.test(value.operatorId) ||
    !/^dev_[A-Za-z0-9_-]{8,64}$/u.test(value.developerIdentityId) ||
    value.email.length < 3 ||
    value.email.length > 320 ||
    value.displayName.length < 1 ||
    value.displayName.length > 200 ||
    (value.developerStatus !== null && !ALLOWED_DEVELOPER_STATUSES.has(value.developerStatus)) ||
    value.permissions.length === 0 ||
    value.permissions.length > ALLOWED_PERMISSIONS.size ||
    value.permissions.some((permission) => !ALLOWED_PERMISSIONS.has(permission)) ||
    !Number.isFinite(expiresAt) ||
    !Number.isFinite(passwordVerifiedAt) ||
    expiresAt <= now ||
    expiresAt - now > 3_600_000 ||
    passwordVerifiedAt > now + 5_000 ||
    passwordVerifiedAt > expiresAt
  ) {
    throw operatorAuthenticationError("Operator session is invalid or expired.");
  }
  return {
    expiresAt: value.expiresAt,
    passwordVerifiedAt: value.passwordVerifiedAt,
    permissions: [...new Set(value.permissions)],
    profile: {
      id: value.operatorId,
      developerIdentityId: value.developerIdentityId,
      email: value.email,
      displayName: value.displayName,
      developerStatus: value.developerStatus,
    },
  };
}

function normalizedEndpoint(value: string): string {
  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch {
    throw operatorAuthenticationError("The management endpoint is invalid.");
  }
  if (
    endpoint.username !== "" ||
    endpoint.password !== "" ||
    endpoint.pathname !== "/" ||
    endpoint.search !== "" ||
    endpoint.hash !== "" ||
    (endpoint.protocol !== "https:" &&
      !(endpoint.protocol === "http:" && ["127.0.0.1", "localhost"].includes(endpoint.hostname)))
  ) {
    throw operatorAuthenticationError("The management endpoint is invalid.");
  }
  return endpoint.origin;
}

function safeOperatorAuthenticationError(error: unknown): Error {
  if (error instanceof ManagementApiError) {
    if (error.status === 429) {
      return operatorAuthenticationError("Too many attempts. Try again later.");
    }
    if (error.status === 401) {
      return operatorAuthenticationError("The email or password was not accepted.");
    }
  }
  return operatorAuthenticationError("Operator authentication is unavailable.");
}

function operatorAuthenticationError(message: string): Error {
  const error = new Error(message);
  error.name = "OperatorAuthenticationError";
  return error;
}
