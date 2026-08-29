import { generateDemoHousehold } from "../../scripts/demo-data.mjs";
import { FAKE_CONFIG } from "../config.js";
import { canonicalJson } from "../data/conflict.js";
import { requestUrl } from "../data/transport.js";
import { emailDigest, invitationId, membershipId } from "../model/ids.js";
import type { BaseDocument, CollectionId, HouseholdRole } from "../model/types.js";

type WireDocument = BaseDocument & { _deleted: boolean } & Record<string, unknown>;

/** The account the fake provider authenticates. */
const PROVIDER_ACCOUNT = "pat.provider@rational.test";

interface FakeUser {
  readonly id: string;
  readonly email: string;
  password: string;
  authorizationEpoch: number;
  /**
   * Which generation of sessions this user may still renew. A role change
   * moves the authorization epoch — the access tokens stop verifying and the
   * app renews to learn its new claims — but a revocation moves this, and
   * then there is nothing left to renew with.
   */
  revocation: number;
  households: Record<string, HouseholdRole>;
}

interface Change {
  readonly sequence: number;
  readonly document: WireDocument;
}

interface CollectionStore {
  readonly documents: Map<string, WireDocument>;
  readonly changes: Change[];
  readonly streams: Set<{
    controller: ReadableStreamDefaultController<Uint8Array>;
    userId: string;
  }>;
}

/**
 * A deterministic, in-browser implementation of the public Mako auth and
 * replication protocol for every Rational collection, with household
 * visibility decided by the same claims the real policies read. It makes the
 * app runnable with no server and the browser suite hermetic; nothing in it
 * is a storage adapter or a production path.
 */
export class FakeMakoBackend {
  readonly #collections = new Map<CollectionId, CollectionStore>();
  readonly #users = new Map<string, FakeUser>();
  /** Magic-link tokens, redeemable once. */
  readonly #magicLinks = new Map<string, { email: string; used: boolean }>();
  #sequence = 0;
  #links = 0;
  #clock = 1_800_000_000_000;
  #lastMagicLink: string | null = null;
  readonly demoHouseholdId = "hh_demo";

  constructor() {
    const demo = generateDemoHousehold({
      householdId: this.demoHouseholdId,
      transactionCount: 60,
    });
    this.#record("households", { ...demo.household, _deleted: false });
    for (const account of demo.accounts) this.#record("accounts", { ...account, _deleted: false });
    for (const entry of demo.taxonomy) {
      this.#record("taxonomy", { ...entry, _deleted: false });
    }
    for (const transaction of demo.transactions) {
      this.#record("transactions", { ...transaction, _deleted: false });
    }
  }

  readonly now = (): number => this.#clock;

  readonly fetch: typeof globalThis.fetch = async (input, init = {}) => {
    const url = requestUrl(input);
    const path = url.pathname;
    const functionPrefix = `/${FAKE_CONFIG.projectId}--${FAKE_CONFIG.environmentId}/functions/v1/households/`;
    if (path.startsWith(functionPrefix)) {
      return this.#households(path.slice(functionPrefix.length), init);
    }
    if (!path.startsWith(`/v1/projects/${FAKE_CONFIG.projectId}/environments/`)) {
      return apiError(404, "not_found", "route not found");
    }
    if (path.endsWith("/auth/signup")) return this.#signUp(init);
    if (path.endsWith("/auth/signin")) return this.#signIn(init);
    if (path.endsWith("/auth/token")) return this.#refresh(init);
    if (path.endsWith("/auth/signout")) return new Response(null, { status: 204 });
    if (path.endsWith("/auth/providers/exchange")) return this.#exchangeProviderCode(init);
    if (path.endsWith("/auth/magic-link")) return this.#requestMagicLink(init);
    if (path.endsWith("/auth/magic-link/redeem")) return this.#redeemMagicLink(init);
    const provider = /\/auth\/providers\/([a-z0-9-]+)\/start$/u.exec(path);
    if (provider !== null) return this.#startProviderSignIn(provider[1] as string, init);

    const match = /\/collections\/([a-z_]+)\/replication\/(pull|push|stream)$/u.exec(path);
    if (match === null) return apiError(404, "not_found", "route not found");
    const collectionId = match[1] as CollectionId;
    const user = this.#authorize(init);
    if (user === null) return apiError(401, "unauthenticated", "the session is not valid");
    switch (match[2]) {
      case "pull":
        return this.#pull(collectionId, user, init);
      case "push":
        return this.#push(collectionId, user, init);
      default:
        return this.#stream(collectionId, user, init.signal ?? undefined, url);
    }
  };

  // --- test hooks -------------------------------------------------------

  advanceClock(milliseconds: number): void {
    this.#clock += milliseconds;
  }

  /** The link the last `requestMagicLink` would have emailed, or null. */
  lastMagicLink(): string | null {
    return this.#lastMagicLink;
  }

  /** The household role a user holds, as their next token would carry it. */
  roleOf(email: string, householdId: string): HouseholdRole | null {
    return this.#requireUser(email).households[householdId] ?? null;
  }

  /** Write a document as some other member's device would. */
  putRemote(collectionId: CollectionId, document: BaseDocument): void {
    this.#record(collectionId, { ...structuredClone(document), _deleted: false } as WireDocument);
  }

  deleteRemote(collectionId: CollectionId, id: string, updatedAt: number): void {
    const current = this.#store(collectionId).documents.get(id);
    if (current === undefined) throw new Error(`${collectionId}/${id} does not exist`);
    this.#record(collectionId, { ...current, updated_at: updatedAt, _deleted: true });
  }

  remoteDocument(collectionId: CollectionId, id: string): WireDocument | undefined {
    const document = this.#store(collectionId).documents.get(id);
    return document === undefined ? undefined : structuredClone(document);
  }

  /**
   * Change a user's household role, as the households function would. The
   * user's authorization epoch advances, their tokens stop verifying, and
   * every open stream learns that the epoch moved.
   */
  setRole(email: string, householdId: string, role: HouseholdRole | null): void {
    this.#applyRole(this.#requireUser(email), householdId, role);
  }

  /** Make the user's sessions permanently unusable. */
  revokeAccess(email: string): void {
    // Every token this user holds names the epoch it was issued at, so moving
    // the epoch is what stops all of them at once — which is how the platform
    // revokes too.
    const user = this.#requireUser(email);
    user.authorizationEpoch += 1;
    user.revocation += 1;
    this.disconnectStreams();
  }

  disconnectStreams(): void {
    for (const store of this.#collections.values()) {
      for (const stream of store.streams) {
        stream.controller.error(new TypeError("simulated stream disconnect"));
      }
      store.streams.clear();
    }
  }

  // --- auth -------------------------------------------------------------

  async #signUp(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ email?: string; password?: string }>(init);
    const email = body.email?.trim().toLowerCase() ?? "";
    const password = body.password ?? "";
    if (!email.includes("@") || password.length < 8) {
      return apiError(400, "invalid_request", "email and a password of 8+ characters are required");
    }
    if (this.#users.has(email)) {
      return apiError(409, "conflict", "an account with that email already exists");
    }
    this.#ensureUser(email, password);
    return jsonResponse({ accepted: true });
  }

  async #signIn(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ email?: string; password?: string }>(init);
    const user = this.#users.get(body.email?.trim().toLowerCase() ?? "");
    const password = body.password ?? "";
    if (user === undefined || password.length === 0) {
      return apiError(401, "unauthenticated", "email or password is incorrect");
    }
    // A user this instance reconstructed from a token it was shown has no
    // password on file — the registration happened in a page that is gone —
    // so the first one offered becomes theirs. A registered one must match.
    if (user.password === "") {
      user.password = password;
    } else if (user.password !== password) {
      return apiError(401, "unauthenticated", "email or password is incorrect");
    }
    return jsonResponse(this.#session(user));
  }

  async #refresh(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ refreshToken?: string }>(init);
    const user = this.#tokenHolder("refresh", body.refreshToken ?? "");
    if (user === null) return apiError(401, "unauthenticated", "the session was revoked");
    return jsonResponse(this.#session(user));
  }

  #session(user: FakeUser): object {
    return {
      accessToken: sessionToken("access", user),
      refreshToken: sessionToken("refresh", user),
      expiresIn: 3_600,
      user: {
        id: user.id,
        email: user.email,
        status: "active",
        authorizationEpoch: user.authorizationEpoch,
      },
    };
  }

  #authorize(init: RequestInit): FakeUser | null {
    const header = new Headers(init.headers).get("authorization") ?? "";
    return this.#tokenHolder("access", header.replace(/^Bearer\s+/iu, ""));
  }

  /**
   * The user a session token names, at the epoch it was issued for.
   *
   * A token minted before a reload names somebody this instance has never
   * seen — it lives in the page, and the page is new — so the user is
   * reconstructed the way first sight reconstructs them. That is what lets a
   * signed-in person reload Rational and keep syncing. An access token from
   * before a role change, or a refresh token from before a revocation, names
   * a generation that has since moved, and is refused as the platform
   * refuses it.
   */
  #tokenHolder(kind: "access" | "refresh", value: string): FakeUser | null {
    const match = SESSION_TOKEN.exec(value);
    if (match === null || match[1] !== kind) return null;
    const email = match[2] ?? "";
    const epoch = Number(match[3]);
    const user = this.#users.get(email) ?? this.#ensureUser(email);
    const current = kind === "access" ? user.authorizationEpoch : user.revocation;
    return current === epoch ? user : null;
  }

  #requireUser(email: string): FakeUser {
    const user = this.#users.get(email.trim().toLowerCase());
    if (user === undefined) throw new Error(`no fake user ${email}`);
    return user;
  }

  // --- provider and magic-link sign-in ----------------------------------

  /**
   * The provider's authorization URL comes back to the app's own redirect
   * with a one-time code, which is what a real provider callback does after
   * the person approves it. The query keeps it a real navigation rather than
   * a fragment change the page would not reload for.
   */
  async #startProviderSignIn(provider: string, init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ redirectUrl?: string }>(init);
    if (provider !== "demo-idp") {
      return apiError(400, "invalid_request", `provider ${provider} is not enabled`);
    }
    const redirect = body.redirectUrl ?? "";
    if (!redirect.startsWith("http")) {
      return apiError(400, "invalid_request", "redirectUrl must be an absolute URL");
    }
    // The code names the account the provider would have authenticated. It
    // says so out loud because the browser leaves this page to "the
    // provider" and comes back to a freshly loaded app — and a fake that
    // lives in the page has no memory across that.
    const code = `${provider}~${PROVIDER_ACCOUNT}`;
    const separator = redirect.includes("?") ? "&" : "?";
    return jsonResponse({
      authorizationUrl: `${redirect}${separator}provider=${provider}#code=${encodeURIComponent(code)}`,
      provider,
    });
  }

  async #exchangeProviderCode(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ code?: string }>(init);
    const [provider, email] = (body.code ?? "").split("~");
    if (provider !== "demo-idp" || email === undefined || !email.includes("@")) {
      return apiError(401, "unauthenticated", "the sign-in code is not valid");
    }
    return jsonResponse(this.#session(this.#ensureUser(email)));
  }

  async #requestMagicLink(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ email?: string; redirectUrl?: string }>(init);
    const email = body.email?.trim().toLowerCase() ?? "";
    const redirect = body.redirectUrl ?? "";
    if (!email.includes("@") || !redirect.startsWith("http")) {
      return apiError(400, "invalid_request", "email and an absolute redirectUrl are required");
    }
    this.#links += 1;
    const token = `magic-token-${this.#links}`;
    this.#magicLinks.set(token, { email, used: false });
    this.#lastMagicLink = `${redirect}#magic_link_token=${token}`;
    // The service never says whether the address is registered.
    return jsonResponse({ accepted: true }, 202);
  }

  async #redeemMagicLink(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ token?: string }>(init);
    const entry = this.#magicLinks.get(body.token ?? "");
    if (entry === undefined || entry.used) {
      return apiError(401, "unauthenticated", "this link has already been used");
    }
    entry.used = true;
    return jsonResponse(this.#session(this.#ensureUser(entry.email)));
  }

  /**
   * A user of this environment, created on first sight. Every new user joins
   * the demo household so there is data to show; the first one owns it, as
   * the households function would have made them.
   */
  #ensureUser(email: string, password = ""): FakeUser {
    const normalized = email.trim().toLowerCase();
    const existing = this.#users.get(normalized);
    if (existing !== undefined) return existing;
    const user: FakeUser = {
      id: `usr_${normalized.replaceAll(/[^a-z0-9]/gu, "").slice(0, 16)}${this.#users.size}`,
      email: normalized,
      password,
      authorizationEpoch: 1,
      revocation: 1,
      households: { [this.demoHouseholdId]: this.#users.size === 0 ? "owner" : "editor" },
    };
    this.#users.set(normalized, user);
    const at = this.#clock;
    this.#record("memberships", {
      id: membershipId(this.demoHouseholdId, user.id),
      household_id: this.demoHouseholdId,
      created_at: at,
      updated_at: at,
      user_id: user.id,
      email: normalized,
      role: user.households[this.demoHouseholdId] ?? "editor",
      status: "active",
      _deleted: false,
    });
    return user;
  }

  // --- the households function ------------------------------------------

  /**
   * The routes of `functions/households/index.ts`, with the same rules: the
   * caller is the verified user, the role comes from the caller's own claim,
   * and every membership write moves a claim (advancing the authorization
   * epoch) and the `memberships` projection together.
   */
  async #households(route: string, init: RequestInit): Promise<Response> {
    const caller = this.#authorize(init);
    if (caller === null) return apiError(401, "unauthenticated", "the session is not valid");
    const body = await jsonBody<Record<string, unknown>>(init);
    const householdId = typeof body.householdId === "string" ? body.householdId : "";
    const owns = () => caller.households[householdId] === "owner";
    switch (route) {
      case "create": {
        const name = typeof body.name === "string" ? body.name.trim() : "";
        const currency = typeof body.currency === "string" ? body.currency : "USD";
        if (name === "") return apiError(400, "invalid_request", "a household needs a name");
        this.#sequence += 1;
        const id = `hh_${this.#sequence.toString(36)}${Date.now().toString(36).slice(-4)}`;
        const at = this.#clock;
        this.#record("households", {
          id,
          household_id: id,
          created_at: at,
          updated_at: at,
          name,
          currency,
          owner_id: caller.id,
          _deleted: false,
        });
        this.#applyRole(caller, id, "owner");
        return jsonResponse({ householdId: id, role: "owner" });
      }
      case "invite": {
        if (!owns()) return apiError(403, "permission_denied", "only the owner may invite");
        const email = (typeof body.email === "string" ? body.email : "").trim().toLowerCase();
        const role = roleOrNull(body.role);
        if (!email.includes("@") || role === null) {
          return apiError(400, "invalid_request", "an email and a role are required");
        }
        const at = this.#clock;
        this.#record("memberships", {
          id: invitationId(householdId, await emailDigest(email)),
          household_id: householdId,
          created_at: at,
          updated_at: at,
          user_id: "",
          email,
          role,
          status: "invited",
          invited_by: caller.id,
          _deleted: false,
        });
        return jsonResponse({ invited: email, role });
      }
      case "accept": {
        const invitation = this.#store("memberships").documents.get(
          invitationId(householdId, await emailDigest(caller.email)),
        );
        if (invitation === undefined || invitation._deleted === true) {
          return apiError(404, "not_found", "there is no invitation for this address");
        }
        this.#record("memberships", {
          ...invitation,
          status: "accepted",
          user_id: caller.id,
          updated_at: this.#clock,
        });
        this.#applyRole(caller, householdId, invitation.role as HouseholdRole);
        return jsonResponse({ householdId, role: invitation.role });
      }
      case "role":
      case "remove": {
        if (!owns()) return apiError(403, "permission_denied", "only the owner may do that");
        const userId = typeof body.userId === "string" ? body.userId : "";
        const target = [...this.#users.values()].find((candidate) => candidate.id === userId);
        if (target === undefined) return apiError(404, "not_found", "no such member");
        const household = this.#store("households").documents.get(householdId);
        if (route === "remove" && household?.owner_id === target.id) {
          return apiError(400, "invalid_request", "the owner cannot be removed");
        }
        const role = route === "remove" ? null : roleOrNull(body.role);
        if (route === "role" && role === null) {
          return apiError(400, "invalid_request", "a role is required");
        }
        this.#applyRole(target, householdId, role);
        return jsonResponse({ householdId, userId, role });
      }
      default:
        return apiError(404, "not_found", "route not found");
    }
  }

  /** Move a claim and the projection together, as the function does. */
  #applyRole(user: FakeUser, householdId: string, role: HouseholdRole | null): void {
    if (role === null) delete user.households[householdId];
    else user.households[householdId] = role;
    // Moving the epoch is what stops the tokens already issued: each one
    // names the epoch it was minted at.
    user.authorizationEpoch += 1;
    const id = membershipId(householdId, user.id);
    const existing = this.#store("memberships").documents.get(id);
    const at = this.#clock;
    this.#record("memberships", {
      ...(existing ?? {
        id,
        household_id: householdId,
        created_at: at,
        user_id: user.id,
        email: user.email,
      }),
      id,
      household_id: householdId,
      updated_at: at,
      user_id: user.id,
      email: user.email,
      role: role ?? (existing?.role as HouseholdRole | undefined) ?? "viewer",
      status: role === null ? "removed" : "active",
      _deleted: false,
    } as WireDocument);
    this.#broadcastResync(user.id, "authorization_epoch_changed");
  }

  // --- replication ------------------------------------------------------

  #canRead(user: FakeUser, collectionId: CollectionId, document: WireDocument): boolean {
    if (user.households[document.household_id] !== undefined) return true;
    // The `invited-read` rule of the memberships policy: a signed-in person
    // can see a pending invitation, which is how they learn of one at all.
    return collectionId === "memberships" && document.status === "invited";
  }

  #canWrite(user: FakeUser, collectionId: CollectionId, document: WireDocument): boolean {
    const role = user.households[document.household_id];
    if (collectionId === "memberships") return false;
    if (collectionId === "households") return role === "owner" || document._deleted === false;
    return role === "owner" || role === "editor";
  }

  async #pull(collectionId: CollectionId, user: FakeUser, init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ checkpoint?: string | null; batchSize?: number }>(init);
    const after = parseCheckpoint(body.checkpoint ?? null);
    const limit = body.batchSize ?? 100;
    const store = this.#store(collectionId);
    const documents: WireDocument[] = [];
    let last = after;
    for (const change of store.changes) {
      if (change.sequence <= after) continue;
      last = change.sequence;
      const visible = this.#visibleState(user, collectionId, change.document);
      if (visible !== null) documents.push(visible);
      if (documents.length >= limit) break;
    }
    return jsonResponse({ documents, checkpoint: checkpoint(last) });
  }

  /** What a user sees of a document: the state, a tombstone, or nothing. */
  #visibleState(
    user: FakeUser,
    collectionId: CollectionId,
    document: WireDocument,
  ): WireDocument | null {
    if (this.#canRead(user, collectionId, document)) return structuredClone(document);
    // Hidden documents arrive as tombstones so a stale local copy is removed.
    return { ...structuredClone(document), _deleted: true };
  }

  async #push(collectionId: CollectionId, user: FakeUser, init: RequestInit): Promise<Response> {
    const body = await jsonBody<{
      rows: Array<{
        mutationId: string;
        assumedMasterState: WireDocument | null;
        newDocumentState: WireDocument;
      }>;
    }>(init);
    const store = this.#store(collectionId);
    const outcomes = body.rows.map((row) => {
      const current = store.documents.get(row.newDocumentState.id);
      const assumed = row.assumedMasterState ?? undefined;
      if (!sameDocument(current, assumed)) {
        return {
          mutationId: row.mutationId,
          status: "conflict",
          masterState:
            current === undefined
              ? { ...row.newDocumentState, _deleted: true }
              : this.#visibleState(user, collectionId, current),
        };
      }
      if (!this.#canWrite(user, collectionId, row.newDocumentState)) {
        return {
          mutationId: row.mutationId,
          status: "denied",
          error: {
            error: {
              code: "permission_denied",
              message: "document mutation is not permitted",
              requestId: "req_rational_fake",
              retry: { kind: "never" },
            },
          },
        };
      }
      this.#record(collectionId, structuredClone(row.newDocumentState));
      return { mutationId: row.mutationId, status: "accepted" };
    });
    return jsonResponse({ outcomes });
  }

  #stream(
    collectionId: CollectionId,
    user: FakeUser,
    signal: AbortSignal | undefined,
    _url: URL,
  ): Response {
    const store = this.#store(collectionId);
    const encoder = new TextEncoder();
    let entry: { controller: ReadableStreamDefaultController<Uint8Array>; userId: string };
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        entry = { controller, userId: user.id };
        store.streams.add(entry);
        controller.enqueue(
          encoder.encode(frame({ event: "heartbeat", data: { cursor: cursor(0) } })),
        );
        signal?.addEventListener(
          "abort",
          () => {
            if (store.streams.delete(entry)) {
              controller.error(new DOMException("stream aborted", "AbortError"));
            }
          },
          { once: true },
        );
      },
      cancel() {
        store.streams.delete(entry);
      },
    });
    return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
  }

  #broadcastResync(userId: string, reason: string): void {
    for (const store of this.#collections.values()) {
      for (const stream of [...store.streams]) {
        if (stream.userId !== userId) continue;
        stream.controller.enqueue(
          new TextEncoder().encode(frame({ event: "resync", data: { reason } })),
        );
      }
    }
  }

  #record(collectionId: CollectionId, document: WireDocument): void {
    this.#sequence += 1;
    const store = this.#store(collectionId);
    const snapshot = structuredClone(document);
    store.documents.set(snapshot.id, snapshot);
    store.changes.push({ sequence: this.#sequence, document: snapshot });
    for (const stream of store.streams) {
      const user = [...this.#users.values()].find((candidate) => candidate.id === stream.userId);
      const visible = user === undefined ? null : this.#visibleState(user, collectionId, snapshot);
      if (visible === null) continue;
      stream.controller.enqueue(
        new TextEncoder().encode(
          frame({
            event: "documents",
            data: {
              documents: [visible],
              checkpoint: checkpoint(this.#sequence),
              cursor: cursor(this.#sequence),
            },
          }),
        ),
      );
    }
  }

  #store(collectionId: CollectionId): CollectionStore {
    let store = this.#collections.get(collectionId);
    if (store === undefined) {
      store = { documents: new Map(), changes: [], streams: new Set() };
      this.#collections.set(collectionId, store);
    }
    return store;
  }
}

async function jsonBody<T>(init: RequestInit): Promise<T> {
  if (typeof init.body !== "string") throw new TypeError("expected a JSON request body");
  return JSON.parse(init.body) as T;
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function apiError(status: number, code: string, message: string): Response {
  return jsonResponse(
    { error: { code, message, requestId: "req_rational_fake", retry: { kind: "never" } } },
    status,
  );
}

function roleOrNull(value: unknown): HouseholdRole | null {
  return value === "owner" || value === "editor" || value === "viewer" ? value : null;
}

function frame(event: unknown): string {
  return `data: ${JSON.stringify(event)}\n\n`;
}

/**
 * A session token that outlives the page it was issued in. This backend lives
 * in the browser, so a reload builds a new one with no memory of the sessions
 * the old one handed out; a token that carries who it is for and which
 * generation it belongs to can still be verified by the new instance, the way
 * the provider's one-time code already is.
 */
const SESSION_TOKEN = /^(access|refresh)~([^~]+)~(\d+)$/u;

function sessionToken(kind: "access" | "refresh", user: FakeUser): string {
  // An access token is bound to the claims it was issued with; a refresh
  // token outlives them, because renewing is how the app finds out its claims
  // moved. Only a revocation ends both.
  return `${kind}~${user.email}~${kind === "access" ? user.authorizationEpoch : user.revocation}`;
}

function checkpoint(sequence: number): string {
  return `mcp1.${sequence.toString().padStart(16, "0")}`;
}

function cursor(sequence: number): string {
  return `msc1.${sequence.toString().padStart(16, "0")}`;
}

function parseCheckpoint(value: string | null): number {
  if (value === null) return 0;
  const sequence = Number(value.slice(value.lastIndexOf(".") + 1));
  return Number.isSafeInteger(sequence) && sequence >= 0 ? sequence : 0;
}

function sameDocument(left: WireDocument | undefined, right: WireDocument | undefined): boolean {
  if (left === undefined || right === undefined) return left === right;
  return canonicalJson(strip(left)) === canonicalJson(strip(right));
}

function strip(document: WireDocument): Record<string, unknown> {
  const copy: Record<string, unknown> = { ...document };
  delete copy._rev;
  delete copy._meta;
  delete copy._attachments;
  return copy;
}
