import type { MakoResyncReason } from "./live.js";
import { makoReplicationErrorFrom, type MakoReplicationError } from "./replication-error.js";

export type MakoReplicationRecoveryState =
  | { readonly kind: "active" }
  | {
      readonly kind: "schema_migration_required";
      readonly requiredSchemaVersion: number | null;
    }
  | {
      readonly kind: "full_resync_required";
      readonly reason: "checkpoint_expired" | "stream_gap" | "service_failover";
    };

export interface MakoReplicationRecoveryHooks {
  pauseReplication(): Promise<void>;
  onSchemaMigrationRequired(state: {
    readonly requiredSchemaVersion: number | null;
  }): Promise<void> | void;
  onFullResyncRequired(state: {
    readonly reason: "checkpoint_expired" | "stream_gap" | "service_failover";
  }): Promise<void> | void;
}

export interface ReplicationRecoveryStatePersistence {
  load(): Promise<MakoReplicationRecoveryState | null>;
  save(state: MakoReplicationRecoveryState): Promise<void>;
}

export class MemoryReplicationRecoveryStatePersistence
  implements ReplicationRecoveryStatePersistence
{
  #state: MakoReplicationRecoveryState | null = null;

  async load(): Promise<MakoReplicationRecoveryState | null> {
    return this.#state;
  }

  async save(state: MakoReplicationRecoveryState): Promise<void> {
    this.#state = state;
  }
}

export interface MakoReplicationRecoveryCoordinatorOptions {
  readonly persistence?: ReplicationRecoveryStatePersistence;
}

export class MakoReplicationRecoveryCoordinator {
  readonly #hooks: MakoReplicationRecoveryHooks;
  readonly #persistence: ReplicationRecoveryStatePersistence;
  #state: MakoReplicationRecoveryState = { kind: "active" };

  constructor(
    hooks: MakoReplicationRecoveryHooks,
    options: MakoReplicationRecoveryCoordinatorOptions = {},
  ) {
    this.#hooks = hooks;
    this.#persistence = options.persistence ?? new MemoryReplicationRecoveryStatePersistence();
  }

  get state(): MakoReplicationRecoveryState {
    return this.#state;
  }

  /**
   * Restore the state persisted by an earlier run. A restored
   * `schema_migration_required` or `full_resync_required` state means the
   * application must finish that recovery before it starts replication.
   */
  async initialize(): Promise<MakoReplicationRecoveryState> {
    const persisted = await this.#persistence.load();
    if (persisted !== null) {
      this.#state = persisted;
    }
    return this.#state;
  }

  /**
   * Accepts the error RxDB emits on `error$` as well as a raw
   * `MakoReplicationError`: RxDB wraps a handler failure in an `RC_PULL` /
   * `RC_PUSH` error, and a recovery state must not be missed because of it.
   */
  async handleError(rawError: unknown): Promise<boolean> {
    const error = makoReplicationErrorFrom(rawError);
    if (error === null) {
      return false;
    }
    if (error.code === "schema_mismatch") {
      const state = {
        kind: "schema_migration_required" as const,
        requiredSchemaVersion: requiredSchemaVersion(error),
      };
      await this.#hooks.pauseReplication();
      this.#state = state;
      await this.#persistence.save(state);
      await this.#hooks.onSchemaMigrationRequired(state);
      return true;
    }
    if (error.code === "checkpoint_expired") {
      await this.#requireFullResync("checkpoint_expired");
      return true;
    }
    return false;
  }

  async handleResyncReason(reason: MakoResyncReason): Promise<boolean> {
    if (
      reason === "checkpoint_expired" ||
      reason === "stream_gap" ||
      reason === "service_failover"
    ) {
      await this.#requireFullResync(reason);
      return true;
    }
    return false;
  }

  async markActive(): Promise<void> {
    const state = { kind: "active" as const };
    this.#state = state;
    await this.#persistence.save(state);
  }

  async #requireFullResync(
    reason: "checkpoint_expired" | "stream_gap" | "service_failover",
  ): Promise<void> {
    const state = { kind: "full_resync_required" as const, reason };
    await this.#hooks.pauseReplication();
    this.#state = state;
    await this.#persistence.save(state);
    await this.#hooks.onFullResyncRequired(state);
  }
}

function requiredSchemaVersion(error: MakoReplicationError): number | null {
  const raw = error.details?.requiredSchemaVersion;
  const value = typeof raw === "number" ? raw : typeof raw === "string" ? Number(raw) : Number.NaN;
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}
