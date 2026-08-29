import { Dexie } from "dexie";

import { MakoRxdbConfigurationError } from "./config.js";
import type { MakoCheckpoint } from "./pull.js";
import type {
  MakoReplicationRecoveryState,
  ReplicationRecoveryStatePersistence,
} from "./recovery.js";
import type {
  ReplicationSecurityState,
  ReplicationSecurityStatePersistence,
} from "./security-reset.js";

/**
 * The minimal key-value contract replication state is written through. Values
 * are JSON-shaped and returned by value; `get` answers `null` for an absent key.
 */
export interface ReplicationStateStore {
  get(key: string): Promise<unknown>;
  set(key: string, value: unknown): Promise<void>;
  delete(key: string): Promise<void>;
}

export class MemoryReplicationStateStore implements ReplicationStateStore {
  readonly #entries = new Map<string, unknown>();

  async get(key: string): Promise<unknown> {
    const value = this.#entries.get(key);
    return value === undefined ? null : structuredClone(value);
  }

  async set(key: string, value: unknown): Promise<void> {
    this.#entries.set(key, structuredClone(value));
  }

  async delete(key: string): Promise<void> {
    this.#entries.delete(key);
  }

  keys(): readonly string[] {
    return Array.from(this.#entries.keys());
  }
}

export interface DexieReplicationStateStoreOptions {
  /** IndexedDB database name; defaults to `mako-replication-state`. */
  readonly databaseName?: string;
  /** An IndexedDB implementation to use instead of `globalThis.indexedDB`. */
  readonly indexedDB?: IDBFactory;
  readonly IDBKeyRange?: typeof IDBKeyRange;
}

interface StateRow {
  readonly key: string;
  readonly value: unknown;
}

export const DEFAULT_REPLICATION_STATE_DATABASE = "mako-replication-state" as const;

/**
 * Replication state in an IndexedDB database of its own, next to the RxDB
 * Dexie storage that holds the documents. Construction fails closed when no
 * IndexedDB implementation is available rather than silently keeping state
 * in memory.
 */
export class DexieReplicationStateStore implements ReplicationStateStore {
  readonly databaseName: string;
  readonly #db: Dexie;

  constructor(options: DexieReplicationStateStoreOptions = {}) {
    const indexedDB =
      options.indexedDB ?? (globalThis as { indexedDB?: IDBFactory | null }).indexedDB;
    const keyRange =
      options.IDBKeyRange ??
      (globalThis as { IDBKeyRange?: typeof IDBKeyRange | null }).IDBKeyRange;
    if (
      indexedDB === undefined ||
      indexedDB === null ||
      keyRange === undefined ||
      keyRange === null
    ) {
      throw new MakoRxdbConfigurationError(
        "IndexedDB is unavailable; inject an implementation or use MemoryReplicationStateStore",
      );
    }
    this.databaseName = options.databaseName ?? DEFAULT_REPLICATION_STATE_DATABASE;
    this.#db = new Dexie(this.databaseName, { indexedDB, IDBKeyRange: keyRange });
    this.#db.version(1).stores({ state: "key" });
  }

  async get(key: string): Promise<unknown> {
    const row = await this.#table().get(key);
    return row === undefined ? null : row.value;
  }

  async set(key: string, value: unknown): Promise<void> {
    await this.#table().put({ key, value });
  }

  async delete(key: string): Promise<void> {
    await this.#table().delete(key);
  }

  close(): void {
    this.#db.close();
  }

  #table() {
    return this.#db.table<StateRow, string>("state");
  }
}

export interface ReplicationCheckpointPersistence {
  load(): Promise<MakoCheckpoint | null>;
  save(checkpoint: MakoCheckpoint): Promise<void>;
  clear(): Promise<void>;
}

export interface DexieReplicationStatePersistenceScope {
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
}

export interface DexieReplicationStatePersistenceOptions {
  /** The store to write through; defaults to a `DexieReplicationStateStore`. */
  readonly store?: ReplicationStateStore;
  /** Database name for the default store. */
  readonly databaseName?: string;
}

/**
 * Durable replication state for one collection: the last checkpoint, the
 * authorization-epoch security state, and the recovery state. Hand
 * `security` to `MakoAuthorizationEpochCoordinator`, `recovery` to
 * `MakoReplicationRecoveryCoordinator`, and `checkpoint` to the pull handler
 * and live stream; a security reset clears the checkpoint and recovery state
 * through the coordinator.
 */
export class DexieReplicationStatePersistence {
  readonly namespace: string;
  readonly security: ReplicationSecurityStatePersistence;
  readonly recovery: ReplicationRecoveryStatePersistence;
  readonly checkpoint: ReplicationCheckpointPersistence;
  readonly #store: ReplicationStateStore;

  constructor(
    scope: DexieReplicationStatePersistenceScope,
    options: DexieReplicationStatePersistenceOptions = {},
  ) {
    this.namespace = `mako:${scope.projectId}:${scope.environmentId}:${scope.collectionId}`;
    this.#store =
      options.store ??
      new DexieReplicationStateStore(
        options.databaseName === undefined ? {} : { databaseName: options.databaseName },
      );
    this.security = {
      load: async () => parseSecurityState(await this.#store.get(this.#key("security"))),
      save: (state) => this.#store.set(this.#key("security"), state),
      clearReplicationState: () => this.clearReplicationState(),
    };
    this.recovery = {
      load: async () => parseRecoveryState(await this.#store.get(this.#key("recovery"))),
      save: (state) => this.#store.set(this.#key("recovery"), state),
    };
    this.checkpoint = {
      load: async () => parseCheckpoint(await this.#store.get(this.#key("checkpoint"))),
      save: (checkpoint) => this.#store.set(this.#key("checkpoint"), checkpoint),
      clear: () => this.#store.delete(this.#key("checkpoint")),
    };
  }

  /** Forget the checkpoint and recovery state; the security state is replaced by its coordinator. */
  async clearReplicationState(): Promise<void> {
    await this.#store.delete(this.#key("checkpoint"));
    await this.#store.delete(this.#key("recovery"));
  }

  /** Forget everything persisted for this collection. */
  async clear(): Promise<void> {
    await this.clearReplicationState();
    await this.#store.delete(this.#key("security"));
  }

  #key(kind: "security" | "recovery" | "checkpoint"): string {
    return `${this.namespace}:${kind}`;
  }
}

function parseSecurityState(value: unknown): ReplicationSecurityState | null {
  if (!isRecord(value) || !isRecord(value.authorizationEpochs)) {
    return null;
  }
  const { environment, user } = value.authorizationEpochs;
  if (
    !isNonNegativeInteger(environment) ||
    !isNonNegativeInteger(user) ||
    typeof value.replicationIdentifier !== "string" ||
    value.replicationIdentifier.length < 1 ||
    !isNonNegativeInteger(value.generation)
  ) {
    return null;
  }
  return {
    authorizationEpochs: { environment, user },
    replicationIdentifier: value.replicationIdentifier,
    generation: value.generation,
  };
}

function parseRecoveryState(value: unknown): MakoReplicationRecoveryState | null {
  if (!isRecord(value)) {
    return null;
  }
  if (value.kind === "active") {
    return { kind: "active" };
  }
  if (value.kind === "schema_migration_required") {
    const required = value.requiredSchemaVersion;
    if (required !== null && !(isNonNegativeInteger(required) && required > 0)) {
      return null;
    }
    return { kind: "schema_migration_required", requiredSchemaVersion: required };
  }
  if (
    value.kind === "full_resync_required" &&
    (value.reason === "checkpoint_expired" ||
      value.reason === "stream_gap" ||
      value.reason === "service_failover")
  ) {
    return { kind: "full_resync_required", reason: value.reason };
  }
  return null;
}

function parseCheckpoint(value: unknown): MakoCheckpoint | null {
  if (!isRecord(value) || typeof value.token !== "string" || value.token.length < 1) {
    return null;
  }
  return { token: value.token };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
