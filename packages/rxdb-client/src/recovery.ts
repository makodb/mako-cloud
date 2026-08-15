import type { MakoResyncReason } from "./live.js";
import { MakoReplicationError } from "./replication-error.js";

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

export class MakoReplicationRecoveryCoordinator {
  readonly #hooks: MakoReplicationRecoveryHooks;
  #state: MakoReplicationRecoveryState = { kind: "active" };

  constructor(hooks: MakoReplicationRecoveryHooks) {
    this.#hooks = hooks;
  }

  get state(): MakoReplicationRecoveryState {
    return this.#state;
  }

  async handleError(error: unknown): Promise<boolean> {
    if (!(error instanceof MakoReplicationError)) {
      return false;
    }
    if (error.code === "schema_mismatch") {
      const state = {
        kind: "schema_migration_required" as const,
        requiredSchemaVersion: requiredSchemaVersion(error),
      };
      await this.#hooks.pauseReplication();
      this.#state = state;
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

  markActive(): void {
    this.#state = { kind: "active" };
  }

  async #requireFullResync(
    reason: "checkpoint_expired" | "stream_gap" | "service_failover",
  ): Promise<void> {
    const state = { kind: "full_resync_required" as const, reason };
    await this.#hooks.pauseReplication();
    this.#state = state;
    await this.#hooks.onFullResyncRequired(state);
  }
}

function requiredSchemaVersion(error: MakoReplicationError): number | null {
  const raw = error.details?.requiredSchemaVersion;
  const value = typeof raw === "number" ? raw : typeof raw === "string" ? Number(raw) : Number.NaN;
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}
