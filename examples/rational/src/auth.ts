import {
  type AuthSessionPersistence,
  type AuthUser,
  BrowserAuthSessionPersistence,
  MakoAuthClient,
  MakoAuthError,
  MakoAuthenticationRequiredError,
  type MakoSignInFragment,
  type MakoUserSession,
  type NormalizedMakoRxdbClientConfig,
} from "@mako-cloud/rxdb";

/**
 * The seam between Rational and application-user authentication: password,
 * external provider, and magic link, all over `MakoAuthClient`, with the
 * package's `BrowserAuthSessionPersistence` keeping the session in
 * `localStorage` so a reload — or an offline restart — finds it again.
 */
export interface RationalAuth {
  /** The client the replication adapters use for access tokens. */
  readonly client: MakoAuthClient;
  restore(): Promise<AuthUser | null>;
  currentUser(): AuthUser | null;
  signUp(email: string, password: string): Promise<void>;
  signIn(email: string, password: string): Promise<AuthUser>;
  /** Where to send the browser for a provider's own sign-in screen. */
  startProviderSignIn(provider: string, redirectUrl: string): Promise<string>;
  /** Exchange the `#code=…` the provider sent the browser back with. */
  completeProviderSignIn(fragment: string): Promise<AuthUser>;
  requestMagicLink(email: string, redirectUrl: string): Promise<void>;
  redeemMagicLink(token: string): Promise<AuthUser>;
  refresh(): Promise<AuthUser>;
  signOut(): Promise<void>;
  /** Whether the last refresh failed because the network was unreachable. */
  readonly refreshUnavailable: boolean;
}

/** What a location fragment carries, classified by the client. */
export function signInFragment(fragment: string): MakoSignInFragment {
  return MakoAuthClient.signInFragment(fragment);
}

export interface MakoRationalAuthOptions {
  readonly fetch: typeof globalThis.fetch;
  readonly persistence?: AuthSessionPersistence;
}

export class MakoRationalAuth implements RationalAuth {
  readonly client: MakoAuthClient;
  readonly #persistence: AuthSessionPersistence;
  #user: AuthUser | null = null;
  #refreshUnavailable = false;
  #refreshInFlight: Promise<AuthUser> | null = null;

  constructor(config: NormalizedMakoRxdbClientConfig, options: MakoRationalAuthOptions) {
    this.#persistence =
      options.persistence ??
      new BrowserAuthSessionPersistence(
        { projectId: config.projectId, environmentId: config.environmentId },
        { key: `rational.auth.${config.projectId}.${config.environmentId}` },
      );
    this.client = new MakoAuthClient(config, {
      persistence: this.#persistence,
      fetch: options.fetch,
    });
  }

  get refreshUnavailable(): boolean {
    return this.#refreshUnavailable;
  }

  async restore(): Promise<AuthUser | null> {
    const session = await this.client.restoreSession();
    this.#user = session?.user ?? null;
    return this.#user;
  }

  currentUser(): AuthUser | null {
    return this.#user;
  }

  async signUp(email: string, password: string): Promise<void> {
    await throughTransient(() => this.client.signUp(email, password));
  }

  async signIn(email: string, password: string): Promise<AuthUser> {
    const session = await throughTransient(() => this.client.signInWithPassword(email, password));
    this.#remember(session);
    return session.user;
  }

  async startProviderSignIn(provider: string, redirectUrl: string): Promise<string> {
    const start = await throughTransient(() =>
      this.client.startProviderSignIn(provider, redirectUrl),
    );
    return start.authorizationUrl;
  }

  async completeProviderSignIn(fragment: string): Promise<AuthUser> {
    const session = await throughTransient(() => this.client.completeProviderSignIn(fragment));
    this.#remember(session);
    return session.user;
  }

  async requestMagicLink(email: string, redirectUrl: string): Promise<void> {
    await throughTransient(() => this.client.requestMagicLink(email, redirectUrl));
  }

  async redeemMagicLink(token: string): Promise<AuthUser> {
    const session = await throughTransient(() => this.client.redeemMagicLink(token));
    this.#remember(session);
    return session.user;
  }

  /**
   * Exchange the refresh credential for a new session. A network failure is
   * not a revocation: the client keeps its stored session and reports the
   * outage, and the app must show that rather than sign anyone out, so only a
   * refusal the server actually made clears the user here.
   */
  async refresh(): Promise<AuthUser> {
    // Refresh credentials rotate on use, so concurrent callers — every
    // replicated scope reacts to the same signal — share one exchange. The
    // client coalesces too; this keeps Rational's own view consistent.
    this.#refreshInFlight ??= this.#refreshOnce().finally(() => {
      this.#refreshInFlight = null;
    });
    return this.#refreshInFlight;
  }

  async #refreshOnce(): Promise<AuthUser> {
    try {
      const session = await this.client.refreshSession();
      this.#remember(session);
      return session.user;
    } catch (error) {
      if (!definitiveRefusal(error)) {
        this.#refreshUnavailable = true;
        throw error;
      }
      this.#user = null;
      throw error;
    }
  }

  async signOut(): Promise<void> {
    try {
      await this.client.signOut();
    } finally {
      this.#user = null;
      this.#refreshUnavailable = false;
    }
  }

  #remember(session: MakoUserSession): void {
    this.#user = session.user;
    this.#refreshUnavailable = false;
  }
}

/**
 * Retry an auth call through a transient outage.
 *
 * A hosted deployment restarts a service for a few seconds when it takes a
 * checkpoint backup, and a `fetch` caught in that window throws a *retryable*
 * `MakoAuthError` (`authentication service is unavailable`) — the base class,
 * not the `MakoAuthenticationRequiredError` subclass a real refusal uses, so
 * the check is on `MakoAuthError.retryable`. The platform's replication
 * already retries through those windows; a person clicking "Sign in" should
 * not be the one to hit the one blip in six minutes. A definitive refusal —
 * a wrong password, an expired code — is never retryable and is surfaced at
 * once. The whole budget is a few seconds, shorter than the window, so a
 * genuine outage still fails promptly rather than hanging.
 */
export async function throughTransient<T>(call: () => Promise<T>): Promise<T> {
  const backoffMs = [400, 900, 1600];
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await call();
    } catch (error) {
      const retryable = error instanceof MakoAuthError && error.retryable;
      if (!retryable || attempt >= backoffMs.length) throw error;
      await new Promise((resolve) => setTimeout(resolve, backoffMs[attempt]));
    }
  }
}

/**
 * Whether the server actually refused the credential. Anything else — an
 * unreachable network, a 429, a 5xx — leaves the session in place and is
 * reported as unavailable.
 */
function definitiveRefusal(error: unknown): boolean {
  if (!(error instanceof MakoAuthenticationRequiredError)) return false;
  if (error.retryable) return false;
  return error.code === "unauthenticated" || error.status === 401 || error.status === 403;
}
