import {
  type AuthorizationEpochResetEvent,
  MakoAuthorizationEpochCoordinator,
  MakoLiveStreamGroup,
  type MakoReplicationActivity,
  MakoReplicationError,
  MakoReplicationRecoveryCoordinator,
  type MakoReplicationRecoveryState,
  type MakoResyncReason,
  type MakoSanitizedReplicationError,
} from "@mako-cloud/rxdb";
import { BehaviorSubject, type Observable } from "rxjs";

import type { CollectionId } from "../model/types.js";
import {
  openDatabase,
  type RationalCollections,
  type RationalDatabase,
  removeDatabase,
} from "./database.js";
import {
  type CollectionReplication,
  makoConfigFor,
  type ReplicationDependencies,
  startCollectionReplication,
} from "./replication.js";
import type { ScopeStatePersistence } from "./replication-state.js";

/**
 * A replicated scope is one local database — the household's, or the user's
 * directory of households — with every collection in it replicating. A
 * session is one generation of that database; the controller owns the
 * current generation together with the security and recovery coordinators
 * that decide when a generation has to be thrown away and started afresh.
 */
export interface ScopeDefinition<Ids extends CollectionId> {
  readonly name: string;
  readonly databaseName: string;
  readonly collectionIds: readonly Ids[];
  /** The household whose documents this scope keeps; unset for the directory. */
  readonly householdId?: string;
  /**
   * Every collection streams, over one connection. A browser has six to a
   * host, so a stream each would starve the pulls and pushes; a group is one.
   */
  readonly pollIntervalMs: number;
}

export interface ScopeState {
  readonly generation: number;
  readonly activity: MakoReplicationActivity;
  readonly recovery: MakoReplicationRecoveryState;
  readonly errors: number;
  readonly lastError: MakoSanitizedReplicationError | null;
  readonly conflicts: number;
  readonly received: number;
  readonly sent: number;
  readonly pendingWrites: number;
  readonly securityResets: number;
  readonly fullResyncs: number;
  readonly streamResyncs: number;
  readonly syncedAt: number | null;
  /** The first pull cycle of this generation completed, so local data reflects the server. */
  readonly initialSynced: boolean;
  readonly notice: string | null;
}

const INITIAL_STATE: ScopeState = {
  generation: 0,
  activity: "idle",
  recovery: { kind: "active" },
  errors: 0,
  lastError: null,
  conflicts: 0,
  received: 0,
  sent: 0,
  pendingWrites: 0,
  securityResets: 0,
  fullResyncs: 0,
  streamResyncs: 0,
  syncedAt: null,
  initialSynced: false,
  notice: null,
};

export class ScopeSession<Ids extends CollectionId> {
  readonly database: RationalDatabase<Ids>;
  readonly replications: ReadonlyMap<Ids, CollectionReplication<unknown>>;
  readonly identifier: string;
  readonly #live: MakoLiveStreamGroup;
  #pollTimer: ReturnType<typeof setInterval> | null = null;

  constructor(
    database: RationalDatabase<Ids>,
    replications: ReadonlyMap<Ids, CollectionReplication<unknown>>,
    identifier: string,
    pollIntervalMs: number,
    live: MakoLiveStreamGroup,
  ) {
    this.database = database;
    this.replications = replications;
    this.identifier = identifier;
    this.#live = live;
    if (pollIntervalMs > 0) {
      this.#pollTimer = setInterval(() => this.reSync(), pollIntervalMs);
    }
  }

  get collections(): RationalCollections<Ids> {
    return this.database.collections;
  }

  /** A collection to query, or null once the database is closing or gone. */
  collection<Id extends Ids>(id: Id): RationalCollections<Ids>[Id] | null {
    if (this.database.closed) return null;
    return (this.database.collections as Partial<RationalCollections<Ids>>)[id] ?? null;
  }

  async pause(): Promise<void> {
    await Promise.all([...this.replications.values()].map((entry) => entry.replication.pause()));
  }

  async resume(): Promise<void> {
    await Promise.all([...this.replications.values()].map((entry) => entry.replication.start()));
    this.reSync();
  }

  reSync(): void {
    for (const entry of this.replications.values()) {
      entry.replication.reSync();
    }
  }

  async awaitInSync(): Promise<void> {
    await Promise.all(
      [...this.replications.values()].map((entry) => entry.replication.awaitInSync()),
    );
  }

  async awaitInitialReplication(): Promise<void> {
    await Promise.all(
      [...this.replications.values()].map((entry) => entry.replication.awaitInitialReplication()),
    );
  }

  async stop(): Promise<void> {
    if (this.#pollTimer !== null) clearInterval(this.#pollTimer);
    this.#pollTimer = null;
    this.#live.close();
    await Promise.all([...this.replications.values()].map((entry) => entry.stop()));
  }

  /** Stop replicating and close the database, keeping its data on disk. */
  async close(): Promise<void> {
    await this.stop();
    await this.database.close();
  }

  /** Stop replicating and erase the database from the device. */
  async remove(): Promise<void> {
    await this.stop();
    await this.database.remove();
  }
}

export interface ScopeControllerOptions<Ids extends CollectionId> {
  readonly definition: ScopeDefinition<Ids>;
  readonly dependencies: ReplicationDependencies;
  /** Durable checkpoints, security epochs, and recovery state for this scope. */
  readonly state: ScopeStatePersistence;
  /** The server no longer accepts the session; the app must sign in again. */
  readonly onAuthenticationRequired: () => void | Promise<void>;
}

export class ScopeController<Ids extends CollectionId> {
  readonly definition: ScopeDefinition<Ids>;
  readonly state$ = new BehaviorSubject<ScopeState>(INITIAL_STATE);
  readonly #dependencies: ReplicationDependencies;
  readonly #onAuthenticationRequired: () => void | Promise<void>;
  readonly #security: MakoAuthorizationEpochCoordinator;
  readonly #recovery: MakoReplicationRecoveryCoordinator;
  readonly #state: ScopeStatePersistence;
  #session: ScopeSession<Ids> | null = null;
  #environmentEpoch = 0;
  #closed = false;
  #refreshInFlight: Promise<void> | null = null;
  #openInFlight: Promise<void> = Promise.resolve();

  constructor(options: ScopeControllerOptions<Ids>) {
    this.definition = options.definition;
    this.#dependencies = options.dependencies;
    this.#onAuthenticationRequired = options.onAuthenticationRequired;
    this.#state = options.state;
    this.#security = new MakoAuthorizationEpochCoordinator(
      `rational:${this.definition.name}:v1`,
      {
        pauseReplication: () => this.#session?.pause() ?? Promise.resolve(),
        clearReplicatedCollection: async ({ replicationRunning }) => {
          const session = this.#session;
          this.#session = null;
          // Tell the screens first so nothing renders from a database being erased.
          this.#patch((state) => ({ ...state, activity: "paused" }));
          // A reset also arrives before this run opened anything — the epochs
          // moved while the person was away — and the previous generation's
          // data is on the device either way. With nothing open there is no
          // handle to erase it through, so it goes by database name.
          if (!replicationRunning || session === null) {
            await session?.remove();
            await removeDatabase(this.definition.databaseName);
          } else {
            await session.remove();
          }
        },
        onSecurityReset: (event: AuthorizationEpochResetEvent) => {
          this.#patch((state) => ({
            ...state,
            securityResets: state.securityResets + 1,
            notice: `Your access changed (${event.reason}); local data was cleared and is syncing again.`,
          }));
        },
        startReplication: async (identifier) => {
          await this.#open(identifier);
        },
      },
      { persistence: this.#state.security },
    );
    this.#recovery = new MakoReplicationRecoveryCoordinator(
      {
        pauseReplication: () => this.#session?.pause() ?? Promise.resolve(),
        onSchemaMigrationRequired: ({ requiredSchemaVersion }) => {
          this.#patch((state) => ({
            ...state,
            recovery: { kind: "schema_migration_required", requiredSchemaVersion },
            activity: "schema_migration_required",
            notice: `This version of Rational needs an update (schema version ${
              requiredSchemaVersion ?? "unknown"
            }).`,
          }));
        },
        onFullResyncRequired: async ({ reason }) => {
          this.#patch((state) => ({
            ...state,
            recovery: { kind: "full_resync_required", reason },
            activity: "full_resync_required",
          }));
          await this.#fullResync(reason);
        },
      },
      { persistence: this.#state.recovery },
    );
  }

  get session(): ScopeSession<Ids> | null {
    return this.#session;
  }

  get state(): ScopeState {
    return this.state$.value;
  }

  observe(): Observable<ScopeState> {
    return this.state$.asObservable();
  }

  async open(userAuthorizationEpoch: number): Promise<void> {
    // Both coordinators read what the previous run persisted: the epochs the
    // local data was replicated under, and a recovery that never finished.
    const recovery = await this.#recovery.initialize();
    const state = await this.#security.initialize({
      environment: this.#environmentEpoch,
      user: userAuthorizationEpoch,
    });
    if (recovery.kind === "schema_migration_required") {
      this.#patch((current) => ({
        ...current,
        recovery,
        activity: "schema_migration_required",
        notice: `This version of Rational needs an update (schema version ${
          recovery.requiredSchemaVersion ?? "unknown"
        }).`,
      }));
      return;
    }
    await this.#open(state.replicationIdentifier);
    if (recovery.kind === "full_resync_required") {
      await this.#fullResync(recovery.reason);
    }
  }

  /** A local write happened; it is pending until the push reports it sent. */
  noteLocalWrite(): void {
    this.#patch((state) => ({ ...state, pendingWrites: state.pendingWrites + 1 }));
  }

  reSync(): void {
    this.#session?.reSync();
  }

  async pause(): Promise<void> {
    await this.#session?.pause();
    this.#patch((state) => ({ ...state, activity: "paused" }));
  }

  async resume(): Promise<void> {
    await this.#session?.resume();
  }

  async awaitInSync(): Promise<void> {
    await this.#session?.awaitInSync();
  }

  /**
   * The platform said the authorization epoch moved: refresh, then reset.
   * When the refreshed session shows this user's own epoch moved, that is the
   * change and it is enough to trigger the reset; the environment epoch is
   * bumped only when the user's did not move, so that an epoch change the app
   * already handled — a membership this person just made — does not reset the
   * same generation twice.
   */
  async handleAuthorizationEpochChanged(): Promise<void> {
    if (this.#closed) return;
    const user = await this.#refreshSession();
    if (user === null) return;
    const stored = this.#security.currentState();
    if (stored !== null && stored.authorizationEpochs.user === user.authorizationEpoch) {
      this.#environmentEpoch += 1;
    }
    await this.#security.handleMismatch({
      environment: this.#environmentEpoch,
      user: user.authorizationEpoch,
    });
    this.#patch((state) => ({ ...state, activity: "active" }));
  }

  /**
   * The session was refreshed and carries this user epoch. Nothing happens
   * when it matches the epoch the local data was replicated under; when it
   * moved — this person's own membership changed — the security coordinator
   * starts a fresh generation, which is what clears a household they left.
   */
  async syncAuthorizationEpoch(userAuthorizationEpoch: number): Promise<void> {
    if (this.#closed) return;
    await this.#security.handleMismatch({
      environment: this.#environmentEpoch,
      user: userAuthorizationEpoch,
    });
  }

  /** Force the security-reset path, as a membership removal would. */
  async forceSecurityReset(): Promise<void> {
    await this.handleAuthorizationEpochChanged();
  }

  async close(): Promise<void> {
    this.#closed = true;
    await this.#session?.close();
    this.#session = null;
    this.state$.complete();
  }

  async remove(): Promise<void> {
    this.#closed = true;
    await this.#session?.remove();
    this.#session = null;
    this.state$.complete();
  }

  /**
   * Generations replace one another, and two of them must never be opening at
   * once: the database has one name, and RxDB refuses to open a name twice.
   */
  async #open(identifier: string): Promise<void> {
    const opening = this.#openInFlight.then(
      () => this.#openGeneration(identifier),
      () => this.#openGeneration(identifier),
    );
    this.#openInFlight = opening;
    await opening;
  }

  async #openGeneration(identifier: string): Promise<void> {
    if (this.#closed) return;
    const replaced = this.#session;
    if (replaced !== null) {
      this.#session = null;
      await replaced.close();
    }
    const definition = this.definition;
    const dependencies = this.#dependencies;
    const database = await openDatabase(definition.databaseName, definition.collectionIds);
    // Opening is asynchronous and the scope may have been closed meanwhile —
    // the person switched households. Leaving this database open would keep
    // the name taken and the next open of it would be refused (RxDB DB8).
    if (this.#closed) {
      await database.close();
      return;
    }
    // One connection for every collection in this scope. A stream each would
    // exhaust the six a browser gives a host before half of them were open,
    // and the pulls and pushes would queue behind the ones that were.
    const live = new MakoLiveStreamGroup(
      definition.collectionIds.map((collectionId) =>
        makoConfigFor(dependencies.config, collectionId, definition.householdId),
      ),
      dependencies.auth.client,
      {
        fetch: dependencies.transport.fetch,
        reconnectMinimumDelayMs: Math.max(10, Math.min(dependencies.config.retryTimeMs, 60_000)),
        reconnectMaximumDelayMs: Math.max(
          40,
          Math.min(dependencies.config.retryTimeMs * 8, 300_000),
        ),
        onResyncReason: (reason) => this.#onResync(reason),
      },
    );
    const replications = new Map<Ids, CollectionReplication<unknown>>();
    for (const collectionId of definition.collectionIds) {
      const collection = database.collections[collectionId];
      const checkpoints = this.#state.checkpointsFor(collectionId);
      const replication = startCollectionReplication(collection, collectionId, dependencies, {
        identifier,
        ...(definition.householdId === undefined ? {} : { householdId: definition.householdId }),
        stream$: live.stream$(collectionId),
        ...(checkpoints === undefined ? {} : { checkpoints }),
        onResync: (reason) => this.#onResync(reason),
        onError: (error) => this.#onError(error),
      });
      replications.set(collectionId, replication as CollectionReplication<unknown>);
      replication.subscription.add(
        replication.signals.activity$.subscribe(() => this.#recomputeActivity()),
      );
      replication.subscription.add(
        replication.signals.received$.subscribe(() =>
          this.#patch((state) => ({
            ...state,
            received: state.received + 1,
            syncedAt: Date.now(),
          })),
        ),
      );
      replication.subscription.add(
        replication.signals.sent$.subscribe(() =>
          this.#patch((state) => ({
            ...state,
            sent: state.sent + 1,
            pendingWrites: Math.max(0, state.pendingWrites - 1),
            syncedAt: Date.now(),
          })),
        ),
      );
      replication.subscription.add(
        replication.signals.conflicts$.subscribe(() =>
          this.#patch((state) => ({ ...state, conflicts: state.conflicts + 1 })),
        ),
      );
      replication.subscription.add(
        replication.signals.errors$.subscribe((error) =>
          this.#patch((state) => ({ ...state, errors: state.errors + 1, lastError: error })),
        ),
      );
    }
    const session = new ScopeSession(
      database,
      replications,
      identifier,
      definition.pollIntervalMs,
      live,
    );
    this.#session = session;
    this.#patch((state) => ({
      ...state,
      generation: state.generation + 1,
      recovery: { kind: "active" },
      activity: "active",
      initialSynced: false,
    }));
    void session.awaitInitialReplication().then(() => {
      if (this.#session === session) {
        this.#patch((state) => ({ ...state, initialSynced: true, syncedAt: Date.now() }));
      }
    });
  }

  async #fullResync(reason: string): Promise<void> {
    const session = this.#session;
    this.#session = null;
    this.#patch((state) => ({ ...state, activity: "paused" }));
    await session?.remove();
    this.#patch((state) => ({
      ...state,
      fullResyncs: state.fullResyncs + 1,
      notice: `Local data was reset (${reason}) and is syncing again from the beginning.`,
    }));
    await this.#state.clearReplicationState();
    await this.#open(`rational:${this.definition.name}:resync-${Date.now().toString(36)}`);
    await this.#recovery.markActive();
  }

  async #onResync(reason: MakoResyncReason): Promise<void> {
    this.#patch((state) => ({ ...state, streamResyncs: state.streamResyncs + 1 }));
    if (reason === "authorization_epoch_changed") {
      await this.handleAuthorizationEpochChanged();
      return;
    }
    await this.#recovery.handleResyncReason(reason);
  }

  async #onError(error: unknown): Promise<void> {
    if (await this.#recovery.handleError(error)) return;
    if (error instanceof MakoReplicationError && error.code === "unauthenticated") {
      const user = await this.#refreshSession();
      if (user !== null) {
        // A refreshed session may carry a new epoch — the platform's way of
        // saying visibility changed — in which case the reset runs here.
        await this.#security.handleMismatch({
          environment: this.#environmentEpoch,
          user: user.authorizationEpoch,
        });
        this.#session?.reSync();
      }
    }
  }

  /**
   * One refresh at a time. A refusal the server confirmed means the session
   * is gone; a refresh the network could not carry means nothing yet.
   */
  async #refreshSession(): Promise<{ authorizationEpoch: number } | null> {
    this.#refreshInFlight ??= (async () => {
      try {
        await this.#dependencies.auth.refresh();
      } catch {
        if (!this.#dependencies.auth.refreshUnavailable) {
          this.#patch((state) => ({ ...state, activity: "authentication_required" }));
          await this.#onAuthenticationRequired();
        }
      }
    })().finally(() => {
      this.#refreshInFlight = null;
    });
    await this.#refreshInFlight;
    return this.#dependencies.auth.currentUser();
  }

  #recomputeActivity(): void {
    const session = this.#session;
    if (session === null) return;
    const blocking = this.state.recovery.kind !== "active";
    if (blocking) return;
    this.#patch((state) => ({ ...state, activity: "active" }));
  }

  #patch(update: (state: ScopeState) => ScopeState): void {
    if (this.state$.closed) return;
    this.state$.next(update(this.state$.value));
  }
}
