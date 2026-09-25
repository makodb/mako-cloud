import {
  type AuthSessionPersistence,
  BrowserAuthSessionPersistence,
  type MakoAuthClient,
} from "@mako-cloud/rxdb";

import type {
  ReferenceBackend,
  ReferenceBackendConfig,
  ReferenceBackendDiagnostics,
} from "./backend.js";
import type { ReferenceTodo } from "./reference-app.js";

/** Everything needed to reach a running deployment and act as a second client. */
export interface LiveBackendOptions extends ReferenceBackendConfig {
  /** The application user. A page that leaves these out asks the person using it. */
  readonly email?: string;
  readonly password?: string;
  /**
   * Register the address before signing in. On by default, because a
   * bootstrapped tenant has no users yet; the page's "Sign in" turns it off so
   * a mistyped address fails instead of quietly creating a new account.
   */
  readonly createAccount?: boolean;
  /**
   * Keep the session in the browser so a reload, or coming back later, stays
   * signed in. Without credentials, `authenticate` then resumes the stored
   * session and fails when there is none, so the page can ask instead.
   */
  readonly rememberSession?: boolean;
  /** Credentials for the second client used by putRemote and deleteRemote. */
  readonly remoteEmail: string;
  readonly remotePassword: string;
  readonly schemaVersion: number;
}

/**
 * Sign-up was accepted where the environment verifies email addresses: the
 * account signs in once the link mailed to it has been opened.
 */
export class VerificationPendingError extends Error {
  constructor(readonly email: string) {
    super(`Check ${email} for a link to confirm the address, then sign in.`);
    this.name = "VerificationPendingError";
  }
}

/** This page, as the place a verification link should land. */
function pageRedirectUrl(): string | undefined {
  const location = globalThis.location;
  if (location === undefined || !/^https?:$/u.test(location.protocol)) return undefined;
  return `${location.origin}${location.pathname}`;
}

/**
 * Redeems the token a verification link opened this page with. Resolves to
 * whether the address is now confirmed; a spent or expired link is `false`.
 */
export async function verifyEmail(config: ReferenceBackendConfig, token: string): Promise<boolean> {
  const response = await globalThis.fetch(
    `${config.endpoint.replace(/\/$/u, "")}/v1/projects/${config.projectId}/environments/${
      config.environmentId
    }/auth/verify-email`,
    {
      method: "POST",
      headers: { "content-type": "application/json", "x-mako-key": config.publicProjectKey },
      body: JSON.stringify({ token }),
    },
  );
  if (response.ok) return true;
  if (response.status === 401) return false;
  throw new Error(`verification failed with status ${response.status}`);
}

interface WireSession {
  accessToken: string;
  refreshToken: string;
}

/**
 * Drives the reference application against a running Mako deployment.
 *
 * Nothing here mocks the protocol: every operation is a real request. Where the
 * fake backend flips a field, this reaches for the equivalent real mechanism —
 * a second authenticated client for remote writes, the refresh endpoint for
 * token rotation, sign-out for revocation, and aborting the in-flight stream
 * request for a reconnect. Offline is the one simulation left, because a
 * browser test cannot unplug a network cable.
 */
export class LiveMakoBackend implements ReferenceBackend {
  readonly config: ReferenceBackendConfig;
  readonly #options: LiveBackendOptions;
  readonly #streamAborts = new Set<AbortController>();
  #acceptedWrites = 0;
  #conflictResponses = 0;
  #online = true;
  #refreshes = 0;
  #remote: WireSession | null = null;
  #streamConnections = 0;

  readonly sessionPersistence: AuthSessionPersistence | undefined;
  readonly persistLocalData: boolean;
  /** The address of the user this backend signed in, once it has. */
  signedInEmail: string | null = null;

  constructor(options: LiveBackendOptions) {
    this.#options = options;
    // A page that remembers who is signed in also keeps their data: the two
    // together are what let someone close the tab and pick up where they were.
    this.persistLocalData = options.rememberSession === true;
    this.sessionPersistence =
      options.rememberSession === true
        ? new BrowserAuthSessionPersistence({
            projectId: options.projectId,
            environmentId: options.environmentId,
          })
        : undefined;
    this.config = {
      endpoint: options.endpoint,
      projectId: options.projectId,
      environmentId: options.environmentId,
      collectionId: options.collectionId,
      publicProjectKey: options.publicProjectKey,
    };
  }

  readonly now = (): number => Date.now();

  readonly fetch: typeof globalThis.fetch = async (input, init = {}) => {
    const url = new URL(
      typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
    );
    if (!this.#online) {
      // The transport is what fails when a device loses connectivity, so this
      // surfaces the same way a real network error does.
      throw new TypeError("simulated offline network");
    }
    if (url.pathname.endsWith("/auth/token")) {
      this.#refreshes += 1;
    }

    if (url.pathname.endsWith("/replication/stream")) {
      // The stream stays open long after its headers arrive, so the controller
      // must remain abortable for the life of the body, not the life of the
      // fetch promise.
      const controller = new AbortController();
      this.#streamAborts.add(controller);
      this.#streamConnections += 1;
      const response = await globalThis.fetch(input, { ...init, signal: controller.signal });
      controller.signal.addEventListener("abort", () => this.#streamAborts.delete(controller));
      return response;
    }

    const response = await globalThis.fetch(input, init);
    if (url.pathname.endsWith("/replication/push") && response.ok) {
      // Read a clone so the caller still receives an unconsumed body.
      const body = (await response.clone().json()) as {
        outcomes?: Array<{ status?: string }>;
      };
      for (const outcome of body.outcomes ?? []) {
        if (outcome.status === "accepted") this.#acceptedWrites += 1;
        if (outcome.status === "conflict") this.#conflictResponses += 1;
      }
    }
    return response;
  };

  async authenticate(auth: MakoAuthClient): Promise<void> {
    const { email, password } = this.#options;
    if (email === undefined || password === undefined) {
      // Resume a remembered session: a refresh token that has expired or been
      // revoked is refused here, and the page asks the person to sign in.
      if (this.sessionPersistence !== undefined && (await auth.restoreSession()) !== null) {
        await auth.validAccessToken();
        this.signedInEmail = auth.currentSession()?.user.email ?? null;
        return;
      }
      throw new Error("an email address and password are required to sign in");
    }
    // Sign-up is idempotent from the caller's point of view: an existing
    // address is accepted and still signs in afterwards. When the person
    // asked for a new account, a refusal is theirs to see; swallowing it only
    // surfaced the sign-in that followed, as a wrong password.
    // The page's own address is where a verification link lands; an
    // environment that does not verify addresses ignores it.
    if (this.#options.createAccount === true) {
      const redirectUrl = pageRedirectUrl();
      const accepted = await auth.signUp(
        email,
        password,
        redirectUrl === undefined ? {} : { redirectUrl },
      );
      if (accepted.verificationRequired === true) {
        throw new VerificationPendingError(email);
      }
    } else if (this.#options.createAccount !== false) {
      await auth.signUp(email, password).catch(() => undefined);
    }
    await auth.signInWithPassword(email, password);
    this.signedInEmail = email;
  }

  diagnostics(): ReferenceBackendDiagnostics {
    return {
      acceptedWrites: this.#acceptedWrites,
      conflictResponses: this.#conflictResponses,
      online: this.#online,
      refreshes: this.#refreshes,
      streamConnections: this.#streamConnections,
    };
  }

  setOnline(online: boolean): void {
    this.#online = online;
    if (!online) this.disconnectStreams();
  }

  async putRemote(document: ReferenceTodo): Promise<void> {
    await this.#pushAsRemoteClient({ ...document, _deleted: false });
  }

  async deleteRemote(id: string, updatedAt: number): Promise<void> {
    const current = await this.#masterState(id);
    await this.#pushAsRemoteClient({
      ...(current ?? { id, ownerId: this.#options.remoteEmail, title: "" }),
      id,
      updatedAt,
      _deleted: true,
    });
  }

  async forceTokenRefresh(auth: MakoAuthClient): Promise<void> {
    await auth.refreshSession();
  }

  disconnectStreams(): void {
    for (const controller of [...this.#streamAborts]) {
      controller.abort();
    }
    this.#streamAborts.clear();
  }

  async revokeAccess(auth: MakoAuthClient): Promise<void> {
    // Signing out invalidates the refresh credential server-side, so the next
    // token exchange genuinely fails rather than being told to fail.
    await auth.signOut();
  }

  /**
   * Write as a genuinely different client: its own application user, its own
   * session, its own replication push. This is what makes a conflict a real
   * conflict rather than a staged one.
   */
  async #pushAsRemoteClient(
    document: Record<string, unknown>,
    attempt = 0,
    knownMasterState?: Record<string, unknown>,
  ): Promise<void> {
    const session = await this.#remoteSession();
    // Replacing an existing document requires declaring the state being
    // replaced. Omitting it is a competing create, which the server rejects as
    // a conflict — correctly, and the way any real second client would be
    // rejected.
    const assumedMasterState =
      knownMasterState ?? (await this.#masterState((document as { id?: string }).id ?? ""));
    const response = await globalThis.fetch(
      `${this.#tenantBase()}/collections/${this.#options.collectionId}/replication/push`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-mako-key": this.#options.publicProjectKey,
          authorization: `Bearer ${session.accessToken}`,
          "idempotency-key": `remote-${crypto.randomUUID()}`,
        },
        body: JSON.stringify({
          schemaVersion: this.#options.schemaVersion,
          rows: [
            {
              mutationId: `remote-${crypto.randomUUID()}`,
              ...(assumedMasterState === undefined ? {} : { assumedMasterState }),
              newDocumentState: document,
            },
          ],
        }),
      },
    );
    if (!response.ok) {
      throw new Error(`remote write failed with status ${response.status}`);
    }
    const body = (await response.json()) as {
      outcomes?: Array<{ status?: string; masterState?: Record<string, unknown> }>;
    };
    const outcome = body.outcomes?.[0];
    if (outcome?.status === "accepted") return;
    // Losing a race is normal for a second client. Resolve against the state the
    // server returned and retry, which is what any real client does.
    if (outcome?.status === "conflict" && outcome.masterState !== undefined && attempt < 3) {
      await this.#pushAsRemoteClient(document, attempt + 1, outcome.masterState);
      return;
    }
    throw new Error(`remote write was not accepted: ${JSON.stringify(body)}`);
  }

  /** The document as the server currently holds it, in replication wire form. */
  async #masterState(id: string): Promise<Record<string, unknown> | undefined> {
    const session = await this.#remoteSession();
    let checkpoint: unknown;
    for (let page = 0; page < 20; page += 1) {
      const response = await globalThis.fetch(
        `${this.#tenantBase()}/collections/${this.#options.collectionId}/replication/pull`,
        {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-mako-key": this.#options.publicProjectKey,
            authorization: `Bearer ${session.accessToken}`,
          },
          body: JSON.stringify({
            schemaVersion: this.#options.schemaVersion,
            batchSize: 100,
            ...(checkpoint === undefined ? {} : { checkpoint }),
          }),
        },
      );
      if (!response.ok) {
        throw new Error(`remote read failed with status ${response.status}`);
      }
      const body = (await response.json()) as {
        documents?: Array<Record<string, unknown>>;
        checkpoint?: unknown;
      };
      const documents = body.documents ?? [];
      const match = documents.find((candidate) => (candidate as { id?: string }).id === id);
      if (match !== undefined) return match;
      if (documents.length < 100) return undefined;
      checkpoint = body.checkpoint;
    }
    return undefined;
  }

  #tenantBase(): string {
    return `${this.#options.endpoint.replace(/\/$/u, "")}/v1/projects/${
      this.#options.projectId
    }/environments/${this.#options.environmentId}`;
  }

  async #remoteSession(): Promise<WireSession> {
    if (this.#remote !== null) return this.#remote;
    const base = this.#tenantBase();
    const headers = {
      "content-type": "application/json",
      "x-mako-key": this.#options.publicProjectKey,
    };
    const credentials = JSON.stringify({
      email: this.#options.remoteEmail,
      password: this.#options.remotePassword,
    });
    await globalThis
      .fetch(`${base}/auth/signup`, { method: "POST", headers, body: credentials })
      .catch(() => undefined);
    const response = await globalThis.fetch(`${base}/auth/signin`, {
      method: "POST",
      headers,
      body: credentials,
    });
    if (!response.ok) {
      throw new Error(`remote client sign-in failed with status ${response.status}`);
    }
    this.#remote = (await response.json()) as WireSession;
    return this.#remote;
  }
}
