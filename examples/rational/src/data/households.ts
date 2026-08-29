import type { RationalAuth } from "../auth.js";
import type { HouseholdRole } from "../model/types.js";

/**
 * Rational's own `households` edge function, from the browser. Membership is
 * a claim on a token, and only trusted code may write a claim, so every
 * membership change goes through the function: it verifies the caller, checks
 * the caller's role, writes the claim with a service credential, and keeps
 * the `memberships` projection in step. The app never writes a membership
 * document itself — the policy would refuse it anyway.
 */
export class HouseholdsError extends Error {
  override readonly name = "HouseholdsError";
  readonly status: number | undefined;
  readonly code: string | undefined;

  constructor(
    message: string,
    options: { status?: number | undefined; code?: string | undefined } = {},
  ) {
    super(message);
    this.status = options.status;
    this.code = options.code;
  }
}

export interface CreateHouseholdResult {
  readonly householdId: string;
  readonly role: HouseholdRole;
}

export interface HouseholdsClient {
  create(input: {
    readonly name: string;
    readonly currency: string;
  }): Promise<CreateHouseholdResult>;
  invite(input: {
    readonly householdId: string;
    readonly email: string;
    readonly role: HouseholdRole;
  }): Promise<void>;
  accept(input: { readonly householdId: string }): Promise<void>;
  setRole(input: {
    readonly householdId: string;
    readonly userId: string;
    readonly role: HouseholdRole;
  }): Promise<void>;
  remove(input: { readonly householdId: string; readonly userId: string }): Promise<void>;
}

export interface HouseholdsClientOptions {
  readonly url: string;
  readonly auth: RationalAuth;
  readonly fetch: typeof globalThis.fetch;
}

export class MakoHouseholdsClient implements HouseholdsClient {
  readonly #url: string;
  readonly #auth: RationalAuth;
  readonly #fetch: typeof globalThis.fetch;

  constructor(options: HouseholdsClientOptions) {
    this.#url = options.url.replace(/\/$/u, "");
    this.#auth = options.auth;
    this.#fetch = options.fetch;
  }

  async create(input: { name: string; currency: string }): Promise<CreateHouseholdResult> {
    return this.#post<CreateHouseholdResult>("create", input);
  }

  async invite(input: { householdId: string; email: string; role: HouseholdRole }): Promise<void> {
    await this.#post<unknown>("invite", input);
  }

  async accept(input: { householdId: string }): Promise<void> {
    await this.#post<unknown>("accept", input);
  }

  async setRole(input: {
    householdId: string;
    userId: string;
    role: HouseholdRole;
  }): Promise<void> {
    await this.#post<unknown>("role", input);
  }

  async remove(input: { householdId: string; userId: string }): Promise<void> {
    await this.#post<unknown>("remove", input);
  }

  async #post<T>(route: string, body: unknown): Promise<T> {
    let accessToken: string;
    try {
      accessToken = await this.#auth.client.validAccessToken();
    } catch {
      throw new HouseholdsError("Sign in again to change a household.", {
        code: "unauthenticated",
      });
    }
    let response: Response;
    try {
      response = await this.#fetch(`${this.#url}/${route}`, {
        method: "POST",
        headers: {
          Accept: "application/json",
          Authorization: `Bearer ${accessToken}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(body),
      });
    } catch {
      throw new HouseholdsError("Households are unavailable while you are offline.", {
        code: "unavailable",
      });
    }
    const payload: unknown = await response.json().catch(() => null);
    if (!response.ok) {
      const error = errorOf(payload);
      throw new HouseholdsError(error.message, { status: response.status, code: error.code });
    }
    return payload as T;
  }
}

function errorOf(payload: unknown): { message: string; code: string | undefined } {
  if (typeof payload === "object" && payload !== null && "error" in payload) {
    const error = (payload as { error: unknown }).error;
    if (typeof error === "object" && error !== null) {
      const record = error as { message?: unknown; code?: unknown };
      return {
        message: typeof record.message === "string" ? record.message : "The request was refused.",
        code: typeof record.code === "string" ? record.code : undefined,
      };
    }
  }
  return { message: "The request was refused.", code: undefined };
}
