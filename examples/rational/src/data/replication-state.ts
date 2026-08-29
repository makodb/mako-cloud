import {
  DexieReplicationStatePersistence,
  DexieReplicationStateStore,
  MemoryReplicationStateStore,
  type ReplicationCheckpointPersistence,
  type ReplicationRecoveryStatePersistence,
  type ReplicationSecurityStatePersistence,
  type ReplicationStateStore,
} from "@mako-cloud/rxdb";

import type { CollectionId } from "../model/types.js";

/**
 * Durable replication state for one scope. The package persists per
 * collection; a Rational scope is a whole database, so this keeps one
 * `DexieReplicationStatePersistence` per collection for its checkpoint plus
 * one for the scope itself, whose security and recovery state cover the
 * generation as a whole.
 *
 * The point of persisting it is what a person notices: after a restart the
 * live stream resumes at the checkpoint it reached instead of re-reading the
 * household, and a security reset — a membership that changed — throws the
 * checkpoints away with the data, so the new generation starts clean.
 */
export interface ScopeStatePersistenceScope {
  readonly projectId: string;
  readonly environmentId: string;
  /** Unique per household (or per user, for the directory). */
  readonly scopeName: string;
  readonly collectionIds: readonly CollectionId[];
}

export class ScopeStatePersistence {
  readonly security: ReplicationSecurityStatePersistence;
  readonly recovery: ReplicationRecoveryStatePersistence;
  readonly #scope: DexieReplicationStatePersistence;
  readonly #collections: ReadonlyMap<CollectionId, DexieReplicationStatePersistence>;

  constructor(
    scope: ScopeStatePersistenceScope,
    store: ReplicationStateStore = createReplicationStateStore(),
  ) {
    const persistenceFor = (collectionId: string) =>
      new DexieReplicationStatePersistence(
        {
          projectId: scope.projectId,
          environmentId: scope.environmentId,
          collectionId: `${scope.scopeName}:${collectionId}`,
        },
        { store },
      );
    this.#scope = persistenceFor("$scope");
    this.#collections = new Map(
      scope.collectionIds.map((collectionId) => [collectionId, persistenceFor(collectionId)]),
    );
    this.recovery = this.#scope.recovery;
    this.security = {
      load: () => this.#scope.security.load(),
      save: (state) => this.#scope.security.save(state),
      // The coordinator calls this after the collection was cleared: the
      // checkpoints of every collection of the generation go with it.
      clearReplicationState: () => this.clearReplicationState(),
    };
  }

  checkpointsFor(collectionId: CollectionId): ReplicationCheckpointPersistence | undefined {
    return this.#collections.get(collectionId)?.checkpoint;
  }

  /** Forget every checkpoint and the recovery state; keep the security state. */
  async clearReplicationState(): Promise<void> {
    await this.#scope.clearReplicationState();
    for (const persistence of this.#collections.values()) {
      await persistence.clearReplicationState();
    }
  }
}

/**
 * One IndexedDB database holds the replication state of every scope. A
 * browser that refuses IndexedDB (a private window with storage blocked)
 * keeps it in memory instead: replication still works, it just cannot resume
 * across a restart.
 */
export function createReplicationStateStore(): ReplicationStateStore {
  try {
    return new DexieReplicationStateStore({ databaseName: "rational-replication-state" });
  } catch {
    return new MemoryReplicationStateStore();
  }
}
