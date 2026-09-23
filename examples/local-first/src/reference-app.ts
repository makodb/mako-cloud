import {
  createMakoPullOptions,
  createMakoPushOptions,
  MakoAuthClient,
  type MakoCheckpoint,
  MakoLivePullStream,
  type MakoReplicationActivity,
  MakoReplicationSignals,
  normalizeMakoRxdbConfig,
} from "@mako-cloud/rxdb";
import {
  createRxDatabase,
  type RxCollection,
  type RxDatabase,
  type RxJsonSchema,
  type WithDeleted,
} from "rxdb";
import { type RxReplicationState, replicateRxCollection } from "rxdb/plugins/replication";
import { getRxStorageMemory } from "rxdb/plugins/storage-memory";
import { RXDB_VERSION } from "rxdb/plugins/utils";
import type { Subscription } from "rxjs";

import type { ReferenceBackend, ReferenceBackendDiagnostics } from "./backend.js";
import { FakeMakoBackend } from "./mock-backend.js";

export interface ReferenceTodo {
  readonly id: string;
  readonly ownerId: string;
  readonly title: string;
  readonly updatedAt: number;
}

export interface ReferenceApplicationDiagnostics extends ReferenceBackendDiagnostics {
  readonly activity: MakoReplicationActivity;
  readonly conflicts: number;
  readonly errors: number;
  readonly lastError: string | null;
  readonly received: number;
  readonly reconnects: number;
}

interface ReferenceCollections {
  todos: RxCollection<ReferenceTodo>;
}

export interface ReferenceApplication {
  readonly backend: ReferenceBackend;
  readonly collection: RxCollection<ReferenceTodo>;
  addTodo(document: ReferenceTodo): Promise<void>;
  close(): Promise<void>;
  deleteTodo(id: string): Promise<void>;
  diagnostics(): ReferenceApplicationDiagnostics;
  forceReconnect(): Promise<void>;
  forceTokenRefresh(): Promise<void>;
  listTodos(): Promise<ReferenceTodo[]>;
  observeTodos(listener: (todos: readonly ReferenceTodo[]) => void): Subscription;
  putRemote(document: ReferenceTodo): Promise<void>;
  removeRemote(id: string, updatedAt: number): Promise<void>;
  revokeAccess(): Promise<void>;
  setOnline(online: boolean): Promise<void>;
  updateTodo(id: string, title: string, updatedAt: number): Promise<void>;
  waitForSync(): Promise<void>;
}

/** How eagerly the live stream reconnects and RxDB retries a failed handler. */
export interface ReplicationTiming {
  readonly reconnectMinimumDelayMs: number;
  readonly reconnectMaximumDelayMs: number;
  /** RxDB repeats a failed pull or push handler on this fixed interval. */
  readonly retryTime: number;
}

/**
 * Against a real deployment. The server rate-limits, and RxDB repeats a failed
 * handler on `retryTime` whatever the failure was, so retrying every few
 * milliseconds turns one refused request into a storm of 429s that feeds
 * itself: the page flickers between offline and online and never settles.
 * These are the client's own reconnect defaults plus the one-second delay a
 * 429 asks for.
 */
export const LIVE_REPLICATION_TIMING: ReplicationTiming = {
  reconnectMinimumDelayMs: 500,
  reconnectMaximumDelayMs: 30_000,
  retryTime: 1_000,
};

/**
 * Against the in-memory fake backend nothing is rate-limited, and the browser
 * tests wait on reconnects and retries, so these run as fast as the event loop
 * allows.
 */
export const FAKE_BACKEND_TIMING: ReplicationTiming = {
  reconnectMinimumDelayMs: 10,
  reconnectMaximumDelayMs: 40,
  retryTime: 25,
};

const todoSchema: RxJsonSchema<ReferenceTodo> = {
  version: 0,
  primaryKey: "id",
  type: "object",
  properties: {
    id: { type: "string", maxLength: 100 },
    ownerId: { type: "string", maxLength: 100 },
    title: { type: "string", maxLength: 500 },
    updatedAt: {
      type: "integer",
      minimum: 0,
      maximum: 9_007_199_254_740_991,
      multipleOf: 1,
    },
  },
  required: ["id", "ownerId", "title", "updatedAt"],
  indexes: ["updatedAt"],
};

export async function createReferenceApplication(
  backend: ReferenceBackend = new FakeMakoBackend(),
  timing: ReplicationTiming = backend instanceof FakeMakoBackend
    ? FAKE_BACKEND_TIMING
    : LIVE_REPLICATION_TIMING,
): Promise<ReferenceApplication> {
  const config = normalizeMakoRxdbConfig({
    endpoint: backend.config.endpoint,
    projectId: backend.config.projectId,
    environmentId: backend.config.environmentId,
    collectionId: backend.config.collectionId,
    schemaVersion: 1,
    publicProjectKey: backend.config.publicProjectKey,
    rxdbVersion: RXDB_VERSION,
    runtime: "browser",
    pullBatchSize: 100,
    pushBatchSize: 100,
  });
  const auth = new MakoAuthClient(config, { fetch: backend.fetch, now: backend.now });
  await backend.authenticate(auth);

  const database = await createRxDatabase<ReferenceCollections>({
    name: `makoexample${crypto.randomUUID().replaceAll("-", "")}`,
    storage: getRxStorageMemory(),
    multiInstance: false,
    eventReduce: true,
  });
  const collections = await database.addCollections({
    todos: {
      schema: todoSchema,
      conflictHandler: {
        isEqual: sameTodo,
        resolve: async ({ newDocumentState, realMasterState }) =>
          newDocumentState.updatedAt > realMasterState.updatedAt
            ? newDocumentState
            : realMasterState,
      },
    },
  });
  const collection = collections.todos;
  const live = new MakoLivePullStream<ReferenceTodo>(config, auth, {
    fetch: backend.fetch,
    reconnectMinimumDelayMs: timing.reconnectMinimumDelayMs,
    reconnectMaximumDelayMs: timing.reconnectMaximumDelayMs,
  });
  const replication = replicateRxCollection<ReferenceTodo, MakoCheckpoint>({
    replicationIdentifier: "mako-reference-todos-v1",
    collection,
    pull: createMakoPullOptions(config, auth, {
      fetch: backend.fetch,
      stream$: live.stream$,
    }),
    push: createMakoPushOptions(config, auth, { fetch: backend.fetch }),
    live: true,
    retryTime: timing.retryTime,
    waitForLeadership: false,
    toggleOnDocumentVisible: false,
  });
  const signals = new MakoReplicationSignals<ReferenceTodo>();
  const signalSubscription = signals.bind(replication);
  const state = {
    activity: "idle" as MakoReplicationActivity,
    conflicts: 0,
    errors: 0,
    lastError: null as string | null,
    received: 0,
    reconnects: 0,
    revoked: false,
  };
  signalSubscription.add(signals.activity$.subscribe((activity) => (state.activity = activity)));
  signalSubscription.add(signals.conflicts$.subscribe(() => (state.conflicts += 1)));
  signalSubscription.add(
    signals.errors$.subscribe((error) => {
      state.errors += 1;
      state.lastError = `${error.code}: ${error.message}`;
    }),
  );
  signalSubscription.add(signals.received$.subscribe(() => (state.received += 1)));
  signalSubscription.add(
    live.stream$.subscribe((event) => {
      if (event === "RESYNC") {
        state.reconnects += 1;
      }
    }),
  );
  live.start();
  await replication.awaitInitialReplication();

  return new ReferenceApplicationImpl(
    backend,
    auth,
    database,
    collection,
    replication,
    live,
    signals,
    signalSubscription,
    state,
  );
}

class ReferenceApplicationImpl implements ReferenceApplication {
  readonly #auth: MakoAuthClient;
  readonly #database: RxDatabase<ReferenceCollections>;
  readonly #live: MakoLivePullStream<ReferenceTodo>;
  readonly #replication: RxReplicationState<ReferenceTodo, MakoCheckpoint>;
  readonly #signals: MakoReplicationSignals<ReferenceTodo>;
  readonly #state: {
    activity: MakoReplicationActivity;
    conflicts: number;
    errors: number;
    lastError: string | null;
    received: number;
    reconnects: number;
    revoked: boolean;
  };
  readonly #subscription: Subscription;

  constructor(
    readonly backend: ReferenceBackend,
    auth: MakoAuthClient,
    database: RxDatabase<ReferenceCollections>,
    readonly collection: RxCollection<ReferenceTodo>,
    replication: RxReplicationState<ReferenceTodo, MakoCheckpoint>,
    live: MakoLivePullStream<ReferenceTodo>,
    signals: MakoReplicationSignals<ReferenceTodo>,
    subscription: Subscription,
    state: {
      activity: MakoReplicationActivity;
      conflicts: number;
      errors: number;
      lastError: string | null;
      received: number;
      reconnects: number;
      revoked: boolean;
    },
  ) {
    this.#auth = auth;
    this.#database = database;
    this.#replication = replication;
    this.#live = live;
    this.#signals = signals;
    this.#subscription = subscription;
    this.#state = state;
  }

  async addTodo(document: ReferenceTodo): Promise<void> {
    this.#assertAvailable();
    await this.collection.insert(document);
  }

  async updateTodo(id: string, title: string, updatedAt: number): Promise<void> {
    this.#assertAvailable();
    const document = await this.collection.findOne(id).exec();
    if (document === null) {
      throw new Error(`todo ${id} does not exist`);
    }
    await document.incrementalPatch({ title, updatedAt });
  }

  async deleteTodo(id: string): Promise<void> {
    this.#assertAvailable();
    const document = await this.collection.findOne(id).exec();
    if (document === null) {
      throw new Error(`todo ${id} does not exist`);
    }
    // One write, one push. Two writes in quick succession race the live
    // stream: after an accepted push RxDB assumes the master state is what it
    // pushed, which carries no server revision, and the second push is refused
    // as invalid until the stream has delivered the new revision.
    await document.incrementalRemove();
  }

  async listTodos(): Promise<ReferenceTodo[]> {
    if (this.#state.revoked) {
      return [];
    }
    const documents = await this.collection.find({ selector: {}, sort: [{ id: "asc" }] }).exec();
    return documents.map((document) => document.toJSON());
  }

  observeTodos(listener: (todos: readonly ReferenceTodo[]) => void): Subscription {
    return this.collection
      .find({ selector: {}, sort: [{ id: "asc" }] })
      .$.subscribe((documents) => listener(documents.map((document) => document.toJSON())));
  }

  diagnostics(): ReferenceApplicationDiagnostics {
    return {
      ...this.backend.diagnostics(),
      activity: this.#state.activity,
      conflicts: this.#state.conflicts,
      errors: this.#state.errors,
      lastError: this.#state.lastError,
      received: this.#state.received,
      reconnects: this.#state.reconnects,
    };
  }

  async setOnline(online: boolean): Promise<void> {
    this.#assertAvailable();
    if (!online) {
      await this.#replication.pause();
      this.backend.setOnline(false);
      this.#signals.reportActivity("paused");
      return;
    }
    this.backend.setOnline(true);
    await this.#replication.start();
    this.#replication.reSync();
  }

  async waitForSync(): Promise<void> {
    this.#assertAvailable();
    await this.#replication.awaitInSync();
  }

  async putRemote(document: ReferenceTodo): Promise<void> {
    this.#assertAvailable();
    await this.backend.putRemote(document);
    if (this.backend.diagnostics().online) {
      this.#replication.reSync();
    }
  }

  async removeRemote(id: string, updatedAt: number): Promise<void> {
    this.#assertAvailable();
    await this.backend.deleteRemote(id, updatedAt);
    this.#replication.reSync();
  }

  async forceTokenRefresh(): Promise<void> {
    this.#assertAvailable();
    await this.backend.forceTokenRefresh(this.#auth);
  }

  async forceReconnect(): Promise<void> {
    this.#assertAvailable();
    const before = this.backend.diagnostics().streamConnections;
    this.backend.disconnectStreams();
    await waitUntil(() => this.backend.diagnostics().streamConnections > before);
  }

  async revokeAccess(): Promise<void> {
    this.#assertAvailable();
    await this.backend.revokeAccess(this.#auth);
    try {
      await this.#auth.validAccessToken();
    } catch {
      this.#state.revoked = true;
      this.#state.activity = "authentication_required";
      await this.#replication.cancel();
      this.#live.close();
      await this.#database.remove();
      return;
    }
    throw new Error("the simulated access revocation was not enforced");
  }

  async close(): Promise<void> {
    this.#subscription.unsubscribe();
    this.#signals.complete();
    this.#live.close();
    await this.#replication.cancel();
    if (!this.#state.revoked) {
      await this.#database.remove();
    }
  }

  #assertAvailable(): void {
    if (this.#state.revoked) {
      throw new Error("authentication is required before local data can be used");
    }
  }
}

function sameTodo(left: WithDeleted<ReferenceTodo>, right: WithDeleted<ReferenceTodo>): boolean {
  return (
    left.id === right.id &&
    left.ownerId === right.ownerId &&
    left.title === right.title &&
    left.updatedAt === right.updatedAt &&
    left._deleted === right._deleted
  );
}

async function waitUntil(predicate: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!predicate()) {
    if (Date.now() >= deadline) {
      throw new Error("timed out waiting for the live stream to reconnect");
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
