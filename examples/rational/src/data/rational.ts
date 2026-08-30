import { type AuthUser, MakoAuthError } from "@mako-cloud/rxdb";
import { BehaviorSubject, type Subscription } from "rxjs";

import { MakoRationalAuth, type RationalAuth, signInFragment } from "../auth.js";
import { functionUrl, type RationalConfig, redirectUrl } from "../config.js";
import { databaseName } from "../model/ids.js";
import {
  type CollectionId,
  DIRECTORY_COLLECTIONS,
  type DirectoryCollectionId,
  HOUSEHOLD_COLLECTIONS,
  type Household,
  type HouseholdCollectionId,
  type HouseholdRole,
  type Membership,
} from "../model/types.js";
import { HouseholdsError, type HouseholdsClient, MakoHouseholdsClient } from "./households.js";
import { makoConfigFor } from "./replication.js";
import { createReplicationStateStore, ScopeStatePersistence } from "./replication-state.js";
import {
  type ScopeController as Controller,
  ScopeController,
  type ScopeSession,
  type ScopeState,
} from "./scope.js";
import { Receipts } from "./receipts.js";
import { Transport, type TransportCounters } from "./transport.js";
import { HouseholdWrites } from "./writes.js";

export type Connectivity = "online" | "offline" | "unreachable";

export interface AppState {
  readonly phase: "starting" | "signed_out" | "ready";
  readonly user: AuthUser | null;
  readonly memberships: readonly Membership[];
  /** Pending invitations addressed to the signed-in person's email. */
  readonly invitations: readonly Membership[];
  readonly households: readonly Household[];
  readonly currentHouseholdId: string | null;
  readonly connectivity: Connectivity;
  readonly browserOnline: boolean;
  readonly simulatedOnline: boolean;
  readonly authError: string | null;
  /** The address a magic link was just sent to, so the screen can say so. */
  readonly magicLinkSentTo: string | null;
  readonly notice: string | null;
  readonly directory: ScopeState | null;
  readonly household: ScopeState | null;
  /** Changes whenever the household session object is replaced. */
  readonly generation: number;
}

export interface Diagnostics extends TransportCounters {
  readonly phase: AppState["phase"];
  readonly connectivity: Connectivity;
  readonly online: boolean;
  readonly currentHouseholdId: string | null;
  readonly memberships: number;
  readonly directory: ScopeState | null;
  readonly household: ScopeState | null;
  readonly activity: string;
  readonly pendingWrites: number;
  readonly received: number;
  readonly sent: number;
  readonly conflicts: number;
  readonly errors: number;
  readonly lastError: string | null;
  readonly securityResets: number;
  readonly fullResyncs: number;
  readonly streamResyncs: number;
}

export interface RationalAppOptions {
  readonly config: RationalConfig;
  readonly fetch?: typeof globalThis.fetch;
  readonly auth?: (transport: Transport) => RationalAuth;
  readonly now?: () => number;
}

const HOUSEHOLD_KEY = "rational.household";
/**
 * The application object: authentication, the user's directory of households,
 * the household currently open, connectivity, and the test hooks. The UI
 * renders `state$`; the browser tests drive the same object as `window.rational`.
 */
export class RationalApp {
  readonly config: RationalConfig;
  readonly transport: Transport;
  readonly auth: RationalAuth;
  readonly state$: BehaviorSubject<AppState>;
  readonly #now: () => number;
  #directory: Controller<DirectoryCollectionId> | null = null;
  #household: Controller<HouseholdCollectionId> | null = null;
  #writes: HouseholdWrites | null = null;
  #receipts: Receipts | null = null;
  readonly #stateStore = createReplicationStateStore();
  readonly #households: HouseholdsClient | null;
  #subscriptions = new Set<Subscription>();
  #directoryWatch: Subscription | null = null;
  #householdWatch: Subscription | null = null;
  #removeListeners: (() => void) | null = null;

  constructor(options: RationalAppOptions) {
    this.config = options.config;
    this.#now = options.now ?? (() => Date.now());
    this.transport = new Transport(
      options.fetch ?? globalThis.fetch.bind(globalThis),
      !options.config.startOffline,
    );
    this.auth =
      options.auth?.(this.transport) ??
      new MakoRationalAuth(makoConfigFor(this.config, "households"), {
        fetch: this.transport.fetch,
      });
    const householdsUrl = functionUrl(this.config, "households");
    this.#households =
      householdsUrl === null
        ? null
        : new MakoHouseholdsClient({
            url: householdsUrl,
            auth: this.auth,
            fetch: this.transport.fetch,
          });
    this.state$ = new BehaviorSubject<AppState>({
      phase: "starting",
      user: null,
      memberships: [],
      invitations: [],
      households: [],
      currentHouseholdId: null,
      connectivity: this.transport.online && navigator.onLine ? "online" : "offline",
      browserOnline: navigator.onLine,
      simulatedOnline: this.transport.online,
      authError: null,
      magicLinkSentTo: null,
      notice: null,
      directory: null,
      household: null,
      generation: 0,
    });
  }

  get state(): AppState {
    return this.state$.value;
  }

  /** The current household's write helpers, or null before one is open. */
  get writes(): HouseholdWrites | null {
    return this.#writes;
  }

  /**
   * The current household's receipts, or null before one is open. Files are
   * not documents: they live in the storage bucket, and every member of the
   * household may open one because the object carries the household as an
   * attribute the bucket's rules read.
   */
  get receipts(): Receipts | null {
    return this.#receipts;
  }

  get household(): Controller<HouseholdCollectionId> | null {
    return this.#household;
  }

  get directory(): Controller<DirectoryCollectionId> | null {
    return this.#directory;
  }

  async start(): Promise<void> {
    const onConnectivity = () =>
      this.#patch((state) => ({
        ...state,
        browserOnline: navigator.onLine,
        simulatedOnline: this.transport.online,
      }));
    const onBrowserOnline = () => {
      onConnectivity();
      this.#directory?.reSync();
      this.#household?.reSync();
    };
    window.addEventListener("online", onBrowserOnline);
    window.addEventListener("offline", onConnectivity);
    const unsubscribeTransport = this.transport.onChange(onConnectivity);
    this.#removeListeners = () => {
      window.removeEventListener("online", onBrowserOnline);
      window.removeEventListener("offline", onConnectivity);
      unsubscribeTransport();
    };
    // A provider or a magic link brings the browser back with a fragment;
    // finishing it is what turns this load into a signed-in one.
    const onFragment = () => void this.completeSignInFromFragment();
    window.addEventListener("hashchange", onFragment);
    const removeListeners = this.#removeListeners;
    this.#removeListeners = () => {
      window.removeEventListener("hashchange", onFragment);
      removeListeners?.();
    };
    if (await this.completeSignInFromFragment()) return;
    const user = await this.auth.restore();
    if (user === null) {
      this.#patch((state) => ({ ...state, phase: "signed_out" }));
      return;
    }
    await this.#openDirectory(user);
  }

  /**
   * Whether this environment offers a household directory the app can change:
   * the `households` function is the only writer of memberships, so without it
   * the screens can show membership but not change it.
   */
  get householdsAvailable(): boolean {
    return this.#households !== null;
  }

  /**
   * Send the browser to a provider's own sign-in screen. It comes back at
   * `redirectUrl()` with `#code=…` or `#error=…`, which `start()` finishes.
   */
  async signInWithProvider(provider: string): Promise<void> {
    const setting = this.config.signIn.providers.find((candidate) => candidate.name === provider);
    if (setting === undefined || !setting.enabled) {
      this.#patch((state) => ({
        ...state,
        authError: `${setting?.label ?? provider} is not enabled for this environment.`,
      }));
      return;
    }
    this.#patch((state) => ({ ...state, authError: null, magicLinkSentTo: null }));
    try {
      const authorizationUrl = await this.auth.startProviderSignIn(provider, redirectUrl());
      window.location.assign(authorizationUrl);
    } catch (error) {
      this.#patch((state) => ({ ...state, authError: describeAuthError(error) }));
    }
  }

  /** Ask for a single-use sign-in link; the answer never says who is registered. */
  async requestMagicLink(email: string): Promise<void> {
    if (!this.config.signIn.magicLinks) {
      this.#patch((state) => ({
        ...state,
        authError: "Magic links are not enabled for this environment.",
      }));
      return;
    }
    this.#patch((state) => ({ ...state, authError: null, magicLinkSentTo: null }));
    try {
      await this.auth.requestMagicLink(email, redirectUrl());
      this.#patch((state) => ({ ...state, magicLinkSentTo: email }));
    } catch (error) {
      this.#patch((state) => ({ ...state, authError: describeAuthError(error) }));
    }
  }

  /**
   * Finish whatever the location fragment carries: a provider's code, a magic
   * link's token, or a provider's refusal. The fragment is cleared either way
   * so a reload cannot replay a one-time code.
   */
  async completeSignInFromFragment(): Promise<boolean> {
    const hash = window.location.hash;
    const fragment = signInFragment(hash);
    if (fragment.kind === "none") return false;
    clearFragment();
    if (fragment.kind === "error") {
      this.#patch((state) => ({
        ...state,
        phase: "signed_out",
        authError: `Sign-in was refused (${fragment.value}).`,
      }));
      return true;
    }
    try {
      const user =
        fragment.kind === "provider_code"
          ? await this.auth.completeProviderSignIn(hash)
          : await this.auth.redeemMagicLink(fragment.value);
      this.#patch((state) => ({ ...state, authError: null, magicLinkSentTo: null }));
      await this.#openDirectory(user);
    } catch (error) {
      this.#patch((state) => ({
        ...state,
        phase: "signed_out",
        user: null,
        authError: describeFragmentError(error, fragment.kind),
      }));
    }
    return true;
  }

  async signUp(email: string, password: string): Promise<void> {
    this.#patch((state) => ({ ...state, authError: null }));
    try {
      await this.auth.signUp(email, password);
    } catch (error) {
      this.#patch((state) => ({ ...state, authError: describeAuthError(error) }));
      throw error;
    }
  }

  async signIn(email: string, password: string): Promise<void> {
    this.#patch((state) => ({ ...state, authError: null }));
    let user: AuthUser;
    try {
      user = await this.auth.signIn(email, password);
    } catch (error) {
      this.#patch((state) => ({ ...state, authError: describeAuthError(error) }));
      throw error;
    }
    await this.#openDirectory(user);
  }

  async signOut(): Promise<void> {
    await this.#closeHousehold(true);
    await this.#closeDirectory(true);
    try {
      await this.auth.signOut();
    } catch {
      // The local session is gone either way.
    }
    this.#patch((state) => ({
      ...state,
      phase: "signed_out",
      user: null,
      memberships: [],
      invitations: [],
      households: [],
      currentHouseholdId: null,
      household: null,
      directory: null,
      magicLinkSentTo: null,
      notice: null,
    }));
  }

  /**
   * Open a household. Selection is serialized: closing the current database
   * is awaited, and while that is in flight the directory must not react to
   * its own membership updates by choosing a different one — the two would
   * race and the later writer would win at random.
   */
  async selectHousehold(
    householdId: string | null,
    options: { readonly removeCurrent?: boolean } = {},
  ): Promise<void> {
    if (householdId === this.state.currentHouseholdId && this.#household !== null) return;
    const token = ++this.#selectionSequence;
    this.#selecting = true;
    try {
      await this.#select(householdId, token, options);
    } finally {
      if (token === this.#selectionSequence) this.#selecting = false;
    }
  }

  async #select(
    householdId: string | null,
    token: number,
    options: { readonly removeCurrent?: boolean },
  ): Promise<void> {
    await this.#closeHousehold(options.removeCurrent === true);
    if (token !== this.#selectionSequence) return;
    rememberHousehold(householdId);
    this.#patch((state) => ({ ...state, currentHouseholdId: householdId, household: null }));
    const user = this.auth.currentUser();
    if (householdId === null || user === null) return;
    const controller = new ScopeController<HouseholdCollectionId>({
      definition: {
        name: `household-${householdId}`,
        databaseName: databaseName("rational", householdId),
        collectionIds: HOUSEHOLD_COLLECTIONS,
        householdId,
        pollIntervalMs: this.config.mode === "fake" ? 1_000 : 10_000,
      },
      dependencies: { config: this.config, auth: this.auth, transport: this.transport },
      state: this.#scopeState(`household-${householdId}`, HOUSEHOLD_COLLECTIONS),
      onAuthenticationRequired: () => this.#authenticationRequired(),
    });
    this.#household = controller;
    let watchedSession: ScopeSession<HouseholdCollectionId> | null = null;
    this.#householdWatch = controller.observe().subscribe((scope) => {
      const session = controller.session;
      const replaced = session !== watchedSession;
      watchedSession = session;
      if (replaced) {
        this.#writes =
          session === null
            ? null
            : new HouseholdWrites({
                collections: session.collections,
                householdId,
                now: this.#now,
                noteLocalWrite: () => controller.noteLocalWrite(),
              });
        this.#receipts =
          session === null
            ? null
            : new Receipts(this.config, this.auth, this.transport, householdId);
      }
      // The screens remount only when the session object itself changes.
      this.#patch((state) => ({
        ...state,
        household: scope,
        generation: replaced ? state.generation + 1 : state.generation,
      }));
    });
    await controller.open(user.authorizationEpoch);
  }

  /**
   * Create a household and open it. The function makes the caller its owner,
   * which changes the caller's claims, so the session is refreshed and the
   * directory re-opened before the new household can be selected.
   */
  async createHousehold(name: string, currency: string): Promise<string> {
    const households = this.#requireHouseholds();
    const created = await households.create({ name: name.trim(), currency });
    await this.#afterMembershipChange();
    await this.selectHousehold(created.householdId);
    return created.householdId;
  }

  /** Invite someone by email; they accept from their own device. */
  async inviteMember(householdId: string, email: string, role: HouseholdRole): Promise<void> {
    await this.#requireHouseholds().invite({ householdId, email: email.trim(), role });
    await this.#afterMembershipChange();
  }

  /** Accept an invitation addressed to this person's email. */
  async acceptInvitation(householdId: string): Promise<void> {
    await this.#requireHouseholds().accept({ householdId });
    await this.#afterMembershipChange();
    await this.selectHousehold(householdId);
  }

  async changeMemberRole(householdId: string, userId: string, role: HouseholdRole): Promise<void> {
    await this.#requireHouseholds().setRole({ householdId, userId, role });
    await this.#afterMembershipChange();
  }

  async removeMember(householdId: string, userId: string): Promise<void> {
    await this.#requireHouseholds().remove({ householdId, userId });
    await this.#afterMembershipChange();
  }

  /** The role the signed-in person holds in a household, from the projection. */
  roleIn(householdId: string | null): HouseholdRole | null {
    if (householdId === null) return null;
    return (
      this.state.memberships.find((membership) => membership.household_id === householdId)?.role ??
      null
    );
  }

  #requireHouseholds(): HouseholdsClient {
    if (this.#households === null) {
      throw new HouseholdsError(
        "Household management needs the households function; this environment has none deployed.",
        { code: "unavailable" },
      );
    }
    return this.#households;
  }

  /**
   * Every membership write ends here: the function moved a claim, and a claim
   * only reaches the app on a new token. Refreshing brings it, and telling the
   * scopes the epoch they hold lets the security coordinator start a fresh
   * generation when this person's own access changed — which is what erases a
   * household one no longer belongs to.
   */
  async #afterMembershipChange(): Promise<void> {
    await this.refreshSession();
    const user = this.auth.currentUser();
    if (user === null) return;
    // The same epoch change also arrives on the live stream, so the scopes may
    // already be resetting; a reset that has begun elsewhere is not a reason
    // to fail the membership change the person just made.
    await Promise.allSettled([
      this.#directory?.syncAuthorizationEpoch(user.authorizationEpoch),
      this.#household?.syncAuthorizationEpoch(user.authorizationEpoch),
    ]);
  }

  /** Simulate losing and regaining the network. */
  async setOnline(online: boolean): Promise<void> {
    if (!online) {
      this.transport.setOnline(false);
      await this.#directory?.pause();
      await this.#household?.pause();
      return;
    }
    this.transport.setOnline(true);
    await this.#directory?.resume();
    await this.#household?.resume();
  }

  online(): boolean {
    return this.transport.online && navigator.onLine;
  }

  async waitForSync(): Promise<void> {
    await this.#directory?.awaitInSync();
    await this.#household?.awaitInSync();
  }

  /** Break every open live stream so the client has to reconnect. */
  disconnectStreams(): void {
    this.transport.disconnectStreams();
  }

  async refreshSession(): Promise<void> {
    await this.auth.refresh();
    this.#patch((state) => ({ ...state, user: this.auth.currentUser() }));
  }

  /** Run the security-reset path on the open household, as a removal would. */
  async forceSecurityReset(): Promise<void> {
    await this.#household?.forceSecurityReset();
    await this.#directory?.forceSecurityReset();
  }

  diagnostics(): Diagnostics {
    const state = this.state;
    const household = state.household;
    const directory = state.directory;
    const scopes = [household, directory].filter((scope): scope is ScopeState => scope !== null);
    const sum = (pick: (scope: ScopeState) => number) =>
      scopes.reduce((total, scope) => total + pick(scope), 0);
    const lastError = household?.lastError ?? directory?.lastError ?? null;
    return {
      ...this.transport.counters(),
      phase: state.phase,
      connectivity: state.connectivity,
      online: this.online(),
      currentHouseholdId: state.currentHouseholdId,
      memberships: state.memberships.length,
      directory,
      household,
      activity: household?.activity ?? directory?.activity ?? "idle",
      pendingWrites: sum((scope) => scope.pendingWrites),
      received: sum((scope) => scope.received),
      sent: sum((scope) => scope.sent),
      conflicts: sum((scope) => scope.conflicts),
      errors: sum((scope) => scope.errors),
      lastError: lastError === null ? null : `${lastError.code}: ${lastError.message}`,
      securityResets: sum((scope) => scope.securityResets),
      fullResyncs: sum((scope) => scope.fullResyncs),
      streamResyncs: sum((scope) => scope.streamResyncs),
    };
  }

  dismissNotice(): void {
    this.#patch((state) => ({ ...state, notice: null }));
  }

  async close(): Promise<void> {
    this.#removeListeners?.();
    await this.#closeHousehold(false);
    await this.#closeDirectory(false);
    for (const subscription of this.#subscriptions) subscription.unsubscribe();
    this.#subscriptions.clear();
  }

  async #openDirectory(user: AuthUser): Promise<void> {
    const controller = new ScopeController<DirectoryCollectionId>({
      definition: {
        name: `directory-${user.id}`,
        databaseName: databaseName("rational-directory", user.id),
        collectionIds: DIRECTORY_COLLECTIONS,
        pollIntervalMs: this.config.mode === "fake" ? 1_000 : 10_000,
      },
      dependencies: { config: this.config, auth: this.auth, transport: this.transport },
      state: this.#scopeState(`directory-${user.id}`, DIRECTORY_COLLECTIONS),
      onAuthenticationRequired: () => this.#authenticationRequired(),
    });
    this.#directory = controller;
    this.#directoryWatch = controller.observe().subscribe((scope) => {
      this.#patch((state) => ({ ...state, directory: scope }));
      this.#watchMemberships(user.id, user.email);
    });
    this.#patch((state) => ({ ...state, phase: "ready", user, authError: null }));
    await controller.open(user.authorizationEpoch);
  }

  #scopeState(scopeName: string, collectionIds: readonly CollectionId[]): ScopeStatePersistence {
    return new ScopeStatePersistence(
      {
        projectId: this.config.projectId,
        environmentId: this.config.environmentId,
        scopeName,
        collectionIds,
      },
      this.#stateStore,
    );
  }

  #membershipSubscription: Subscription | null = null;
  #watchedGeneration = -1;
  #selectionSequence = 0;
  #selecting = false;

  #watchMemberships(userId: string, email: string): void {
    const controller = this.#directory;
    const session = controller?.session ?? null;
    if (controller === null || session === null) return;
    if (this.#watchedGeneration === controller.state.generation) return;
    this.#watchedGeneration = controller.state.generation;
    this.#membershipSubscription?.unsubscribe();
    const subscription = session.collections.memberships
      .find({ selector: { user_id: userId, status: "active" } })
      .$.subscribe((documents) => {
        const memberships = documents
          .map((document) => document.toJSON())
          .sort((left, right) => left.household_id.localeCompare(right.household_id));
        this.#patch((state) => ({ ...state, memberships }));
        this.#reconcileSelection(memberships);
      });
    // An invitation is a membership document that names an address rather
    // than a user; the person it names is the one who may accept it. The
    // policy scopes the row to that address -- `old.email == identity.email
    // && identity.email_verified` -- so replication delivers only this
    // user's, and the filter here is the client agreeing with the server
    // rather than the thing that keeps other people's addresses private.
    subscription.add(
      session.collections.memberships
        .find({ selector: { status: "invited" } })
        .$.subscribe((documents) => {
          const invitations = documents
            .map((document) => document.toJSON())
            .filter(
              (membership) => (membership.email ?? "").toLowerCase() === email.trim().toLowerCase(),
            )
            .sort((left, right) => left.household_id.localeCompare(right.household_id));
          this.#patch((state) => ({ ...state, invitations }));
        }),
    );
    subscription.add(
      session.collections.households.find().$.subscribe((documents) => {
        const households = documents
          .map((document) => document.toJSON())
          .sort((left, right) => left.name.localeCompare(right.name));
        this.#patch((state) => ({ ...state, households }));
      }),
    );
    this.#membershipSubscription = subscription;
  }

  /**
   * Keep the open household one the user still belongs to.
   *
   * Two states look alike and are not: a membership that was taken away, and
   * one that has not been pulled yet. Right after a security reset the local
   * directory is empty and fills up again, so a household is only abandoned
   * once this generation of the directory has finished its first pull —
   * while a device that has nothing open yet may choose from whatever it has,
   * which is what lets an offline restart show the household from disk.
   */
  #reconcileSelection(memberships: readonly Membership[]): void {
    const directory = this.#directory;
    if (directory === null || this.#selecting) return;
    const ids = memberships.map((membership) => membership.household_id);
    const current = this.state.currentHouseholdId;
    if (current !== null && ids.includes(current)) {
      if (this.#household === null) void this.selectHousehold(current);
      return;
    }
    if (current === null) {
      const remembered = rememberedHousehold();
      const next = remembered !== null && ids.includes(remembered) ? remembered : (ids[0] ?? null);
      if (next !== null) void this.selectHousehold(next);
      return;
    }
    if (!directory.state.initialSynced) return;
    // The membership really is gone: leaving erases the household's local copy.
    const remembered = rememberedHousehold();
    const next = remembered !== null && ids.includes(remembered) ? remembered : (ids[0] ?? null);
    void this.selectHousehold(next, { removeCurrent: true });
  }

  async #authenticationRequired(): Promise<void> {
    if (this.state.phase !== "ready") return;
    await this.#closeHousehold(true);
    await this.#closeDirectory(true);
    this.#patch((state) => ({
      ...state,
      phase: "signed_out",
      user: null,
      memberships: [],
      invitations: [],
      households: [],
      household: null,
      directory: null,
      authError: "Your session ended. Sign in again to continue.",
    }));
  }

  async #closeHousehold(remove: boolean): Promise<void> {
    const controller = this.#household;
    this.#household = null;
    this.#writes = null;
    this.#receipts = null;
    this.#householdWatch?.unsubscribe();
    this.#householdWatch = null;
    if (controller === null) return;
    if (remove) await controller.remove();
    else await controller.close();
  }

  async #closeDirectory(remove: boolean): Promise<void> {
    const controller = this.#directory;
    this.#directory = null;
    this.#membershipSubscription?.unsubscribe();
    this.#membershipSubscription = null;
    this.#watchedGeneration = -1;
    this.#directoryWatch?.unsubscribe();
    this.#directoryWatch = null;
    if (controller === null) return;
    if (remove) await controller.remove();
    else await controller.close();
  }

  #patch(update: (state: AppState) => AppState): void {
    const next = update(this.state$.value);
    this.state$.next({ ...next, connectivity: this.#connectivityFor(next) });
  }

  #connectivityFor(state: AppState): Connectivity {
    if (!state.simulatedOnline || !state.browserOnline) return "offline";
    const scope = state.household ?? state.directory;
    const lastError = scope?.lastError ?? null;
    if (lastError !== null && lastError.code === "unavailable" && scope?.activity !== "active") {
      return "unreachable";
    }
    return "online";
  }
}

/** Take the one-time code out of the address bar without reloading. */
function clearFragment(): void {
  const { pathname, search } = window.location;
  window.history.replaceState(null, "", `${pathname}${search}`);
}

/**
 * A refusal on the way back from a provider or a link says what it means for
 * that method: the password wording would be nonsense here.
 */
function describeFragmentError(error: unknown, kind: "provider_code" | "magic_link"): string {
  if (error instanceof MakoAuthError && error.status === 401) {
    return kind === "magic_link"
      ? "That sign-in link has already been used or has expired."
      : "That provider sign-in could not be completed. Try again.";
  }
  return describeAuthError(error);
}

function describeAuthError(error: unknown): string {
  if (error instanceof MakoAuthError) {
    if (error.status === 401) return "That email and password do not match.";
    if (error.status === 409) return "An account with that email already exists.";
    return error.message;
  }
  return error instanceof Error ? error.message : "Something went wrong.";
}

function rememberHousehold(householdId: string | null): void {
  try {
    if (householdId === null) window.localStorage.removeItem(HOUSEHOLD_KEY);
    else window.localStorage.setItem(HOUSEHOLD_KEY, householdId);
  } catch {
    // Not remembering is fine.
  }
}

function rememberedHousehold(): string | null {
  try {
    return window.localStorage.getItem(HOUSEHOLD_KEY);
  } catch {
    return null;
  }
}
