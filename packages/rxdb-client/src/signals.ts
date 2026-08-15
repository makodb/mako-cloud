import type { RxReplicationConflict } from "rxdb";
import type { RxReplicationState } from "rxdb/plugins/replication";
import { BehaviorSubject, Subject, Subscription, type Observable } from "rxjs";

import type { AuthorizationEpochResetEvent } from "./security-reset.js";
import { MakoReplicationError } from "./replication-error.js";

export type MakoReplicationActivity =
  | "idle"
  | "active"
  | "paused"
  | "authentication_required"
  | "schema_migration_required"
  | "full_resync_required"
  | "stopped";

export interface MakoSanitizedReplicationError {
  readonly code: string;
  readonly message: string;
  readonly requestId: string | null;
  readonly retryable: boolean;
  readonly status: number | null;
}

export interface MakoThrottleSignal {
  readonly code: "rate_limited" | "quota_exceeded";
  readonly retryAfterMilliseconds: number | null;
}

export class MakoReplicationSignals<RxDocType> {
  readonly #activity = new BehaviorSubject<MakoReplicationActivity>("idle");
  readonly #received = new Subject<RxDocType>();
  readonly #sent = new Subject<RxDocType>();
  readonly #conflicts = new Subject<RxReplicationConflict<RxDocType>>();
  readonly #throttling = new Subject<MakoThrottleSignal>();
  readonly #securityResets = new Subject<AuthorizationEpochResetEvent>();
  readonly #errors = new Subject<MakoSanitizedReplicationError>();

  get activity$(): Observable<MakoReplicationActivity> {
    return this.#activity.asObservable();
  }

  get received$(): Observable<RxDocType> {
    return this.#received.asObservable();
  }

  get sent$(): Observable<RxDocType> {
    return this.#sent.asObservable();
  }

  get conflicts$(): Observable<RxReplicationConflict<RxDocType>> {
    return this.#conflicts.asObservable();
  }

  get throttling$(): Observable<MakoThrottleSignal> {
    return this.#throttling.asObservable();
  }

  get securityResets$(): Observable<AuthorizationEpochResetEvent> {
    return this.#securityResets.asObservable();
  }

  get errors$(): Observable<MakoSanitizedReplicationError> {
    return this.#errors.asObservable();
  }

  bind(
    replication: Pick<
      RxReplicationState<RxDocType, unknown>,
      "active$" | "conflict$" | "error$" | "received$" | "sent$"
    >,
  ): Subscription {
    const subscription = new Subscription();
    subscription.add(
      replication.active$.subscribe((active) => this.#activity.next(active ? "active" : "idle")),
    );
    subscription.add(replication.received$.subscribe((document) => this.#received.next(document)));
    subscription.add(replication.sent$.subscribe((document) => this.#sent.next(document)));
    subscription.add(replication.conflict$.subscribe((conflict) => this.#conflicts.next(conflict)));
    subscription.add(replication.error$.subscribe((error) => this.reportError(error)));
    return subscription;
  }

  reportActivity(activity: MakoReplicationActivity): void {
    this.#activity.next(activity);
  }

  reportSecurityReset(event: AuthorizationEpochResetEvent): void {
    this.#securityResets.next(event);
  }

  reportError(error: unknown): void {
    const sanitized = sanitizeError(error);
    this.#errors.next(sanitized);
    if (error instanceof MakoReplicationError) {
      if (error.code === "rate_limited" || error.code === "quota_exceeded") {
        this.#throttling.next({
          code: error.code,
          retryAfterMilliseconds: error.retry.kind === "after_delay" ? error.retry.afterMs : null,
        });
      } else if (error.code === "unauthenticated") {
        this.#activity.next("authentication_required");
      } else if (error.code === "schema_mismatch") {
        this.#activity.next("schema_migration_required");
      } else if (error.code === "checkpoint_expired") {
        this.#activity.next("full_resync_required");
      }
    }
  }

  complete(): void {
    this.#activity.next("stopped");
    this.#activity.complete();
    this.#received.complete();
    this.#sent.complete();
    this.#conflicts.complete();
    this.#throttling.complete();
    this.#securityResets.complete();
    this.#errors.complete();
  }
}

function sanitizeError(error: unknown): MakoSanitizedReplicationError {
  if (error instanceof MakoReplicationError) {
    return {
      code: error.code,
      message: error.message,
      requestId: error.requestId,
      retryable: error.retryable,
      status: error.status ?? null,
    };
  }
  return {
    code: "internal",
    message: "replication operation failed",
    requestId: null,
    retryable: true,
    status: null,
  };
}
