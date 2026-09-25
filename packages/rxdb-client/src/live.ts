import type { RxReplicationPullStreamItem, WithDeleted } from "rxdb";
import { Subject, type Observable } from "rxjs";

import { MakoAuthenticationRequiredError, type MakoAuthClient } from "./auth.js";
import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import type { MakoCheckpoint } from "./pull.js";
import { MakoReplicationError, replicationNetworkError } from "./replication-error.js";
import { sendReplicationRequest } from "./replication-session.js";
import type { ReplicationCheckpointPersistence } from "./replication-state.js";
import type { MakoLiveStreamEvent, MakoResyncReason } from "./wire.js";

export type { MakoResyncReason };

export interface MakoLiveStreamOptions {
  readonly fetch?: typeof globalThis.fetch;
  readonly maximumBufferedEvents?: number;
  readonly reconnectMinimumDelayMs?: number;
  readonly reconnectMaximumDelayMs?: number;
  /**
   * How long an open stream may go without a single byte before it is treated
   * as dead and reconnected. The service sends a heartbeat every 15 seconds,
   * so the default of 45 seconds is three missed heartbeats. A connection that
   * went half-open when a device slept or changed networks, or a proxy that
   * holds the stream back, otherwise leaves the client waiting forever and
   * never learns of remote changes.
   */
  readonly silenceTimeoutMs?: number;
  readonly onResyncReason?: (reason: MakoResyncReason) => Promise<void> | void;
  /** Records every checkpoint the stream advances to so a restart can resume from it. */
  readonly checkpoints?: ReplicationCheckpointPersistence;
}

export class MakoLivePullStream<RxDocType> {
  readonly #config: NormalizedMakoRxdbClientConfig;
  readonly #auth: MakoAuthClient;
  readonly #fetch: typeof globalThis.fetch;
  readonly #maximumBufferedEvents: number;
  readonly #minimumDelay: number;
  readonly #maximumDelay: number;
  readonly #silenceTimeout: number;
  readonly #onResyncReason: ((reason: MakoResyncReason) => Promise<void> | void) | undefined;
  readonly #checkpoints: ReplicationCheckpointPersistence | undefined;
  readonly #subject = new Subject<RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>>();
  readonly #queue: RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>[] = [];
  #abort: AbortController | null = null;
  #reader: ReadableStreamDefaultReader<Uint8Array> | null = null;
  #silence: ReturnType<typeof setTimeout> | undefined;
  #running = false;
  #closed = false;
  #draining = false;
  #checkpoint: MakoCheckpoint | undefined;
  #cursor: string | undefined;

  constructor(
    config: NormalizedMakoRxdbClientConfig,
    auth: MakoAuthClient,
    options: MakoLiveStreamOptions = {},
  ) {
    this.#config = config;
    this.#auth = auth;
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#maximumBufferedEvents = boundedInteger(options.maximumBufferedEvents ?? 100, 1, 1_000);
    this.#minimumDelay = boundedInteger(options.reconnectMinimumDelayMs ?? 500, 10, 60_000);
    this.#maximumDelay = boundedInteger(options.reconnectMaximumDelayMs ?? 30_000, 10, 300_000);
    this.#silenceTimeout = boundedInteger(options.silenceTimeoutMs ?? 45_000, 10, 600_000);
    this.#onResyncReason = options.onResyncReason;
    this.#checkpoints = options.checkpoints;
    if (this.#maximumDelay < this.#minimumDelay) {
      throw new TypeError("reconnectMaximumDelayMs must not be less than the minimum delay");
    }
  }

  get stream$(): Observable<RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>> {
    return this.#subject.asObservable();
  }

  start(checkpoint?: MakoCheckpoint): void {
    if (this.#closed) {
      throw new Error("live stream is closed");
    }
    if (this.#running) {
      return;
    }
    this.#checkpoint = checkpoint;
    this.#running = true;
    void this.#run();
  }

  close(): void {
    this.#closed = true;
    this.#running = false;
    this.#abort?.abort();
    this.#abort = null;
    clearTimeout(this.#silence);
    // Cancelling the body ends a read the abort did not reach, so nothing is
    // left holding the connection or a timer after close.
    void this.#reader?.cancel().catch(ignore);
    this.#reader = null;
    this.#queue.length = 0;
    this.#subject.complete();
  }

  async #run(): Promise<void> {
    let delay = this.#minimumDelay;
    let reconnecting = false;
    while (!this.#closed) {
      if (reconnecting) {
        this.#enqueue("RESYNC");
        await wait(delay);
        delay = Math.min(delay * 2, this.#maximumDelay);
        if (this.#closed) {
          break;
        }
      }
      try {
        await this.#connect();
        delay = this.#minimumDelay;
      } catch (error) {
        if (this.#closed) {
          break;
        }
        // A failure the service said not to repeat -- a refused session above
        // all -- ends the stream rather than reconnecting against a verdict.
        if (error instanceof MakoAuthenticationRequiredError || isTerminalError(error)) {
          this.#subject.error(error);
          this.#closed = true;
          break;
        }
        if (error instanceof Error && error.name === "AbortError") {
          reconnecting = true;
          continue;
        }
        delay = this.#advisedDelay(error, delay);
        reconnecting = true;
        continue;
      }
      reconnecting = true;
    }
    this.#running = false;
  }

  /** Honour an `after_delay` advice for the next reconnect, inside the configured bounds. */
  #advisedDelay(error: unknown, current: number): number {
    const advised = error instanceof MakoReplicationError ? error.retryAfterMilliseconds : null;
    if (advised === null) {
      return current;
    }
    return Math.min(Math.max(advised, this.#minimumDelay), this.#maximumDelay);
  }

  async #connect(): Promise<void> {
    const accessToken = await this.#auth.validAccessToken();
    const abort = new AbortController();
    this.#abort = abort;
    // Any bytes -- a heartbeat included -- prove the stream is alive. After
    // too long without any, drop it: the reconnect asks RxDB to resync, so
    // whatever the stream failed to deliver arrives by pull instead.
    const armSilence = (): void => {
      clearTimeout(this.#silence);
      if (this.#closed) {
        return;
      }
      this.#silence = setTimeout(() => {
        abort.abort();
        void this.#reader?.cancel().catch(ignore);
      }, this.#silenceTimeout);
    };
    armSilence();
    try {
      await this.#readStream(accessToken, abort, armSilence);
    } finally {
      clearTimeout(this.#silence);
      this.#reader = null;
    }
  }

  async #readStream(accessToken: string, abort: AbortController, alive: () => void): Promise<void> {
    const response = await sendReplicationRequest(this.#auth, accessToken, async (token) => {
      try {
        return await this.#fetch(streamUrl(this.#config, this.#checkpoint, this.#cursor), {
          headers: {
            Accept: "text/event-stream",
            Authorization: `Bearer ${token}`,
            "X-Mako-Key": this.#config.publicProjectKey,
          },
          signal: abort.signal,
        });
      } catch (error) {
        if (abort.signal.aborted) {
          throw new DOMException("stream aborted", "AbortError");
        }
        throw error;
      }
    });
    if (response.body === null) {
      throw replicationNetworkError();
    }
    const reader = response.body.getReader();
    this.#reader = reader;
    if (this.#closed) {
      void reader.cancel().catch(ignore);
      return;
    }
    alive();
    const decoder = new TextDecoder();
    let pending = "";
    while (!this.#closed && !abort.signal.aborted) {
      const chunk = await reader.read();
      if (chunk.done) {
        break;
      }
      alive();
      pending += decoder.decode(chunk.value, { stream: true }).replaceAll("\r\n", "\n");
      let boundary = pending.indexOf("\n\n");
      while (boundary >= 0) {
        const frame = pending.slice(0, boundary);
        pending = pending.slice(boundary + 2);
        this.#acceptFrame(frame);
        boundary = pending.indexOf("\n\n");
      }
    }
    // A stream left because the client closed or dropped it is cancelled, so
    // the connection is released rather than kept open behind a lock.
    if (this.#closed || abort.signal.aborted) {
      void reader.cancel().catch(ignore);
    } else {
      reader.releaseLock();
    }
  }

  #acceptFrame(frame: string): void {
    const data = frame
      .split("\n")
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n");
    if (data.length === 0) {
      return;
    }
    let event: unknown;
    try {
      event = JSON.parse(data);
    } catch {
      this.#overflow();
      return;
    }
    if (!isLiveEvent(event)) {
      this.#overflow();
      return;
    }
    if ("cursor" in event.data && typeof event.data.cursor === "string") {
      this.#cursor = event.data.cursor;
    }
    if (event.event === "documents") {
      const checkpoint = { token: event.data.checkpoint };
      this.#advance(checkpoint);
      this.#enqueue({
        documents: event.data.documents as WithDeleted<RxDocType>[],
        checkpoint,
      });
    } else if (event.event === "checkpoint") {
      const checkpoint = { token: event.data.checkpoint };
      this.#advance(checkpoint);
      this.#enqueue({ documents: [], checkpoint });
    } else if (event.event === "resync") {
      if (this.#onResyncReason !== undefined) {
        void this.#onResyncReason(event.data.reason);
      }
      this.#enqueue("RESYNC");
    }
  }

  #advance(checkpoint: MakoCheckpoint): void {
    this.#checkpoint = checkpoint;
    // A persistence failure must not stall the stream: RxDB's own checkpoint
    // stays authoritative for pulls, and a restart only resumes less precisely.
    void this.#checkpoints?.save(checkpoint).catch(ignore);
  }

  #enqueue(event: RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>): void {
    if (this.#queue.length >= this.#maximumBufferedEvents) {
      this.#overflow();
      return;
    }
    this.#queue.push(event);
    if (!this.#draining) {
      this.#draining = true;
      queueMicrotask(() => this.#drain());
    }
  }

  #drain(): void {
    const event = this.#queue.shift();
    if (event !== undefined && !this.#closed) {
      this.#subject.next(event);
    }
    if (this.#queue.length > 0 && !this.#closed) {
      queueMicrotask(() => this.#drain());
    } else {
      this.#draining = false;
    }
  }

  #overflow(): void {
    this.#queue.length = 0;
    this.#queue.push("RESYNC");
    this.#abort?.abort();
    if (!this.#draining) {
      this.#draining = true;
      queueMicrotask(() => this.#drain());
    }
  }
}

export function createMakoLivePullStream<RxDocType>(
  config: NormalizedMakoRxdbClientConfig,
  auth: MakoAuthClient,
  options: MakoLiveStreamOptions = {},
): MakoLivePullStream<RxDocType> {
  return new MakoLivePullStream(config, auth, options);
}

function streamUrl(
  config: NormalizedMakoRxdbClientConfig,
  checkpoint?: MakoCheckpoint,
  cursor?: string,
): URL {
  const base = new URL(config.endpoint);
  base.pathname = `${base.pathname.replace(/\/+$/u, "")}/`;
  const url = new URL(
    `v1/projects/${encodeURIComponent(config.projectId)}/environments/${encodeURIComponent(
      config.environmentId,
    )}/collections/${encodeURIComponent(config.collectionId)}/replication/stream`,
    base,
  );
  url.searchParams.set("schemaVersion", String(config.schemaVersion));
  // The stream has to narrow exactly as the pull does: wider and the local
  // database discards what it is sent, narrower and it never learns of
  // changes the pull would have delivered.
  if (config.filter !== null) {
    url.searchParams.set("filterField", config.filter.field);
    url.searchParams.set("filterValue", config.filter.value);
  }
  if (cursor !== undefined) {
    url.searchParams.set("cursor", cursor);
  } else if (checkpoint !== undefined) {
    url.searchParams.set("checkpoint", checkpoint.token);
  }
  return url;
}

/** A failure the service refused to have repeated: reconnecting cannot fix it. */
export function isTerminalError(error: unknown): boolean {
  return error instanceof MakoReplicationError && !error.retryable;
}

export function isLiveEvent(value: unknown): value is MakoLiveStreamEvent {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as { event?: unknown; data?: unknown };
  if (
    typeof candidate.event !== "string" ||
    typeof candidate.data !== "object" ||
    candidate.data === null
  ) {
    return false;
  }
  const data = candidate.data as Record<string, unknown>;
  if (candidate.event === "documents") {
    return (
      Array.isArray(data.documents) &&
      data.documents.every(isDeletedDocument) &&
      typeof data.checkpoint === "string" &&
      typeof data.cursor === "string"
    );
  }
  if (candidate.event === "checkpoint") {
    return typeof data.checkpoint === "string" && typeof data.cursor === "string";
  }
  if (candidate.event === "heartbeat") {
    return typeof data.cursor === "string";
  }
  return (
    candidate.event === "resync" &&
    [
      "reconnected",
      "stream_gap",
      "checkpoint_expired",
      "authorization_epoch_changed",
      "service_failover",
    ].includes(String(data.reason))
  );
}

function isDeletedDocument(value: unknown): boolean {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { _deleted?: unknown })._deleted === "boolean"
  );
}

export function boundedInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new TypeError(`value must be an integer between ${minimum} and ${maximum}`);
  }
  return value;
}

export function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export function ignore(): void {
  // Intentionally empty: see MakoLivePullStream.#advance.
}
