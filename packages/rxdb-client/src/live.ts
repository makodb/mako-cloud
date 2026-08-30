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
  readonly #onResyncReason: ((reason: MakoResyncReason) => Promise<void> | void) | undefined;
  readonly #checkpoints: ReplicationCheckpointPersistence | undefined;
  readonly #subject = new Subject<RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>>();
  readonly #queue: RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>[] = [];
  #abort: AbortController | null = null;
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
    const decoder = new TextDecoder();
    let pending = "";
    while (!this.#closed && !abort.signal.aborted) {
      const chunk = await reader.read();
      if (chunk.done) {
        break;
      }
      pending += decoder.decode(chunk.value, { stream: true }).replaceAll("\r\n", "\n");
      let boundary = pending.indexOf("\n\n");
      while (boundary >= 0) {
        const frame = pending.slice(0, boundary);
        pending = pending.slice(boundary + 2);
        this.#acceptFrame(frame);
        boundary = pending.indexOf("\n\n");
      }
    }
    reader.releaseLock();
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
