export interface AuthorizationEpochSnapshot {
  readonly environment: number;
  readonly user: number;
}

export interface ReplicationSecurityState {
  readonly authorizationEpochs: AuthorizationEpochSnapshot;
  readonly replicationIdentifier: string;
  readonly generation: number;
}

export interface ReplicationSecurityStatePersistence {
  load(): Promise<ReplicationSecurityState | null>;
  save(state: ReplicationSecurityState): Promise<void>;
  /**
   * Forget the replication state (checkpoint, recovery state) that belonged
   * to the generation being replaced. Called by the coordinator during a
   * security reset, after the collection was cleared and before the new
   * state is saved.
   */
  clearReplicationState?(): Promise<void>;
}

export class MemoryReplicationSecurityStatePersistence
  implements ReplicationSecurityStatePersistence
{
  #state: ReplicationSecurityState | null = null;

  async load(): Promise<ReplicationSecurityState | null> {
    return this.#state === null ? null : structuredClone(this.#state);
  }

  async save(state: ReplicationSecurityState): Promise<void> {
    this.#state = structuredClone(state);
  }
}

export interface AuthorizationEpochResetHooks {
  pauseReplication(): Promise<void>;
  clearReplicatedCollection(): Promise<void>;
  startReplication(replicationIdentifier: string): Promise<void>;
  onSecurityReset(event: AuthorizationEpochResetEvent): Promise<void> | void;
}

export interface AuthorizationEpochResetEvent {
  readonly reason: "authorization_epoch_changed";
  readonly previous: AuthorizationEpochSnapshot;
  readonly current: AuthorizationEpochSnapshot;
  readonly replicationIdentifier: string;
}

export interface AuthorizationEpochCoordinatorOptions {
  readonly persistence?: ReplicationSecurityStatePersistence;
  readonly identifierFactory?: (baseIdentifier: string, generation: number) => string;
}

export class MakoAuthorizationEpochCoordinator {
  readonly #baseIdentifier: string;
  readonly #hooks: AuthorizationEpochResetHooks;
  readonly #persistence: ReplicationSecurityStatePersistence;
  readonly #identifierFactory: (baseIdentifier: string, generation: number) => string;
  #state: ReplicationSecurityState | null = null;
  #resetInFlight: Promise<ReplicationSecurityState> | null = null;

  constructor(
    baseIdentifier: string,
    hooks: AuthorizationEpochResetHooks,
    options: AuthorizationEpochCoordinatorOptions = {},
  ) {
    if (baseIdentifier.length < 1 || baseIdentifier.length > 256) {
      throw new TypeError("base replication identifier is invalid");
    }
    this.#baseIdentifier = baseIdentifier;
    this.#hooks = hooks;
    this.#persistence = options.persistence ?? new MemoryReplicationSecurityStatePersistence();
    this.#identifierFactory = options.identifierFactory ?? secureIdentifier;
  }

  async initialize(
    authorizationEpochs: AuthorizationEpochSnapshot,
  ): Promise<ReplicationSecurityState> {
    validateEpochs(authorizationEpochs);
    const persisted = await this.#persistence.load();
    if (persisted !== null) {
      if (sameEpochs(persisted.authorizationEpochs, authorizationEpochs)) {
        this.#state = persisted;
        return persisted;
      }
      // Durable local data was replicated under other epochs: clear it before
      // adopting the current ones. Nothing is running yet, so there is
      // nothing to pause and the caller starts replication with the returned
      // identifier.
      return this.#reset(persisted, authorizationEpochs, { restart: false });
    }
    const state: ReplicationSecurityState = {
      authorizationEpochs,
      replicationIdentifier: this.#baseIdentifier,
      generation: 0,
    };
    await this.#persistence.save(state);
    this.#state = state;
    return state;
  }

  async handleMismatch(current: AuthorizationEpochSnapshot): Promise<ReplicationSecurityState> {
    validateEpochs(current);
    const state = this.#state ?? (await this.#persistence.load());
    if (state === null) {
      return this.initialize(current);
    }
    this.#state = state;
    if (sameEpochs(state.authorizationEpochs, current)) {
      return state;
    }
    this.#resetInFlight ??= this.#reset(state, current, { restart: true }).finally(() => {
      this.#resetInFlight = null;
    });
    return this.#resetInFlight;
  }

  currentState(): ReplicationSecurityState | null {
    return this.#state;
  }

  async #reset(
    previousState: ReplicationSecurityState,
    current: AuthorizationEpochSnapshot,
    options: { readonly restart: boolean },
  ): Promise<ReplicationSecurityState> {
    if (options.restart) {
      await this.#hooks.pauseReplication();
    }
    await this.#hooks.clearReplicatedCollection();
    await this.#persistence.clearReplicationState?.();
    const generation = previousState.generation + 1;
    const state: ReplicationSecurityState = {
      authorizationEpochs: current,
      replicationIdentifier: this.#identifierFactory(this.#baseIdentifier, generation),
      generation,
    };
    await this.#persistence.save(state);
    this.#state = state;
    await this.#hooks.onSecurityReset({
      reason: "authorization_epoch_changed",
      previous: previousState.authorizationEpochs,
      current,
      replicationIdentifier: state.replicationIdentifier,
    });
    if (options.restart) {
      await this.#hooks.startReplication(state.replicationIdentifier);
    }
    return state;
  }
}

function secureIdentifier(baseIdentifier: string, generation: number): string {
  return `${baseIdentifier}:security-${generation}:${globalThis.crypto.randomUUID()}`;
}

function validateEpochs(epochs: AuthorizationEpochSnapshot): void {
  if (
    !Number.isSafeInteger(epochs.environment) ||
    epochs.environment < 0 ||
    !Number.isSafeInteger(epochs.user) ||
    epochs.user < 0
  ) {
    throw new TypeError("authorization epochs must be non-negative safe integers");
  }
}

function sameEpochs(left: AuthorizationEpochSnapshot, right: AuthorizationEpochSnapshot): boolean {
  return left.environment === right.environment && left.user === right.user;
}
