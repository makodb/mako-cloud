import type { RxReplicationPullStreamItem, WithDeleted } from "rxdb";
import { Subject, type Observable } from "rxjs";

import { MakoAuthenticationRequiredError, type MakoAuthClient } from "./auth.js";
import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import { boundedInteger, ignore, isLiveEvent, isTerminalError, wait } from "./live.js";
import type { MakoLiveStreamOptions } from "./live.js";
import type { MakoCheckpoint } from "./pull.js";
import { MakoReplicationError, replicationNetworkError } from "./replication-error.js";
import { sendReplicationRequest } from "./replication-session.js";
import type { ReplicationCheckpointPersistence } from "./replication-state.js";

/**
 * One live connection carrying several collections.
 *
 * A browser opens six connections to one host. An application with a dozen
 * collections therefore cannot have a stream each: the later streams queue
 * behind the earlier ones, and the pulls and pushes queue behind those, so
 * the application appears connected and syncs nothing. This opens one
 * connection for every collection it is given; each event names the
 * collection it belongs to, and each collection keeps its own checkpoint and
 * cursor, so nothing about sharing a connection changes what a collection
 * receives.
 *
 * A reconnect sends every collection's own cursor back. One `Last-Event-ID`
 * could only speak for whichever event happened to be last, which would leave
 * every other collection resuming from a position it never reached.
 */
export class MakoLiveStreamGroup {
  readonly #auth: MakoAuthClient;
  readonly #fetch: typeof globalThis.fetch;
  readonly #maximumBufferedEvents: number;
  readonly #minimumDelay: number;
  readonly #maximumDelay: number;
  readonly #onResyncReason: ((reason: string) => Promise<void> | void) | undefined;
  readonly #checkpoints: ReplicationCheckpointPersistence | undefined;
  readonly #endpoint: URL;
  readonly #projectId: string;
  readonly #environmentId: string;
  readonly #publicProjectKey: string;
  readonly #collections = new Map<string, StreamedCollection>();
  #abort: AbortController | null = null;
  #running = false;
  #closed = false;
  #draining = false;

  constructor(
    configs: readonly NormalizedMakoRxdbClientConfig[],
    auth: MakoAuthClient,
    options: MakoLiveStreamOptions = {},
  ) {
    const first = configs[0];
    if (first === undefined) {
      throw new TypeError("a live stream group needs at least one collection");
    }
    // Every collection on one connection is one environment's, because the
    // connection is authenticated and routed as that environment's.
    for (const config of configs) {
      if (
        config.endpoint.toString() !== first.endpoint.toString() ||
        config.projectId !== first.projectId ||
        config.environmentId !== first.environmentId ||
        config.publicProjectKey !== first.publicProjectKey
      ) {
        throw new TypeError("every collection on one live stream must share its environment");
      }
      if (this.#collections.has(config.collectionId)) {
        throw new TypeError(`collection ${config.collectionId} is named twice`);
      }
      this.#collections.set(config.collectionId, {
        config,
        subject: new Subject<RxReplicationPullStreamItem<never, MakoCheckpoint>>(),
        queue: [],
      });
    }
    this.#endpoint = first.endpoint;
    this.#projectId = first.projectId;
    this.#environmentId = first.environmentId;
    this.#publicProjectKey = first.publicProjectKey;
    this.#auth = auth;
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#maximumBufferedEvents = boundedInteger(options.maximumBufferedEvents ?? 100, 1, 10_000);
    this.#minimumDelay = boundedInteger(options.reconnectMinimumDelayMs ?? 500, 0, 60_000);
    this.#maximumDelay = boundedInteger(
      options.reconnectMaximumDelayMs ?? 15_000,
      this.#minimumDelay,
      300_000,
    );
    this.#onResyncReason = options.onResyncReason as
      | ((reason: string) => Promise<void> | void)
      | undefined;
    this.#checkpoints = options.checkpoints;
  }

  /** The `pull.stream$` for one of this group's collections. */
  stream$<RxDocType>(
    collectionId: string,
  ): Observable<RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>> {
    const collection = this.#require(collectionId);
    this.#start();
    return collection.subject.asObservable() as Observable<
      RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>
    >;
  }

  /** The checkpoint a collection last advanced to on this connection. */
  checkpointOf(collectionId: string): MakoCheckpoint | undefined {
    return this.#require(collectionId).checkpoint;
  }

  close(): void {
    this.#closed = true;
    this.#abort?.abort();
    for (const collection of this.#collections.values()) {
      collection.subject.complete();
    }
  }

  #require(collectionId: string): StreamedCollection {
    const collection = this.#collections.get(collectionId);
    if (collection === undefined) {
      throw new TypeError(`collection ${collectionId} is not on this live stream`);
    }
    return collection;
  }

  #start(): void {
    if (this.#running || this.#closed) {
      return;
    }
    this.#running = true;
    void this.#run();
  }

  async #run(): Promise<void> {
    let delay = this.#minimumDelay;
    let reconnecting = false;
    while (!this.#closed) {
      if (reconnecting) {
        await wait(delay);
        delay = Math.min(delay === 0 ? this.#minimumDelay : delay * 2, this.#maximumDelay);
      }
      try {
        await this.#connect();
        delay = this.#minimumDelay;
      } catch (error) {
        if (this.#closed) {
          break;
        }
        if (error instanceof MakoAuthenticationRequiredError || isTerminalError(error)) {
          for (const collection of this.#collections.values()) {
            collection.subject.error(error);
          }
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
    const body = JSON.stringify({
      collections: [...this.#collections.values()].map((collection) => ({
        collectionId: collection.config.collectionId,
        schemaVersion: collection.config.schemaVersion,
        ...(collection.cursor === undefined
          ? collection.checkpoint === undefined
            ? {}
            : { checkpoint: collection.checkpoint.token }
          : { cursor: collection.cursor }),
        ...(collection.config.filter === null ? {} : { filter: collection.config.filter }),
      })),
    });
    const response = await sendReplicationRequest(this.#auth, accessToken, async (token) => {
      try {
        return await this.#fetch(this.#streamUrl(), {
          method: "POST",
          headers: {
            Accept: "text/event-stream",
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
            "X-Mako-Key": this.#publicProjectKey,
          },
          body,
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
        this.#acceptFrame(pending.slice(0, boundary));
        pending = pending.slice(boundary + 2);
        boundary = pending.indexOf("\n\n");
      }
    }
    reader.releaseLock();
  }

  #streamUrl(): URL {
    const base = new URL(this.#endpoint);
    base.pathname = `${base.pathname.replace(/\/+$/u, "")}/`;
    return new URL(
      `v1/projects/${encodeURIComponent(this.#projectId)}/environments/${encodeURIComponent(
        this.#environmentId,
      )}/replication/stream`,
      base,
    );
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
      this.#overflowAll();
      return;
    }
    if (!isLiveEvent(event)) {
      this.#overflowAll();
      return;
    }
    // An event that does not say which collection it is for cannot be routed,
    // and guessing would deliver one collection's documents to another. The
    // tag sits beside `event` and `data` rather than inside the payload: the
    // payloads are exactly the single-collection stream's, so the code that
    // reads them is the same either way.
    const named = (event as { collection?: unknown }).collection;
    if (typeof named !== "string") {
      this.#overflowAll();
      return;
    }
    const collection = this.#collections.get(named);
    if (collection === undefined) {
      this.#overflowAll();
      return;
    }
    if ("cursor" in event.data && typeof event.data.cursor === "string") {
      collection.cursor = event.data.cursor;
    }
    if (event.event === "documents") {
      const checkpoint = { token: event.data.checkpoint };
      this.#advance(collection, checkpoint);
      this.#enqueue(collection, {
        documents: event.data.documents as WithDeleted<never>[],
        checkpoint,
      });
    } else if (event.event === "checkpoint") {
      const checkpoint = { token: event.data.checkpoint };
      this.#advance(collection, checkpoint);
      this.#enqueue(collection, { documents: [], checkpoint });
    } else if (event.event === "resync") {
      if (this.#onResyncReason !== undefined) {
        void this.#onResyncReason(event.data.reason);
      }
      this.#enqueue(collection, "RESYNC");
    }
  }

  #advance(collection: StreamedCollection, checkpoint: MakoCheckpoint): void {
    collection.checkpoint = checkpoint;
    void this.#checkpoints?.save(checkpoint).catch(ignore);
  }

  #enqueue(
    collection: StreamedCollection,
    event: RxReplicationPullStreamItem<never, MakoCheckpoint>,
  ): void {
    if (collection.queue.length >= this.#maximumBufferedEvents) {
      this.#overflow(collection);
      return;
    }
    collection.queue.push(event);
    if (!this.#draining) {
      this.#draining = true;
      queueMicrotask(() => this.#drain());
    }
  }

  #drain(): void {
    let delivered = false;
    for (const collection of this.#collections.values()) {
      const event = collection.queue.shift();
      if (event !== undefined && !this.#closed) {
        collection.subject.next(event);
        delivered = true;
      }
    }
    if (delivered && !this.#closed) {
      queueMicrotask(() => this.#drain());
    } else {
      this.#draining = false;
    }
  }

  /**
   * One collection fell behind, so only that collection resyncs -- the
   * connection stays up for the rest. The connection is not aborted here for
   * the same reason: the others are still keeping up.
   */
  #overflow(collection: StreamedCollection): void {
    collection.queue.length = 0;
    collection.queue.push("RESYNC");
    if (!this.#draining) {
      this.#draining = true;
      queueMicrotask(() => this.#drain());
    }
  }

  /** The connection itself is not trustworthy: every collection resyncs. */
  #overflowAll(): void {
    for (const collection of this.#collections.values()) {
      collection.queue.length = 0;
      collection.queue.push("RESYNC");
    }
    this.#abort?.abort();
    if (!this.#draining) {
      this.#draining = true;
      queueMicrotask(() => this.#drain());
    }
  }
}

interface StreamedCollection {
  readonly config: NormalizedMakoRxdbClientConfig;
  readonly subject: Subject<RxReplicationPullStreamItem<never, MakoCheckpoint>>;
  readonly queue: RxReplicationPullStreamItem<never, MakoCheckpoint>[];
  checkpoint?: MakoCheckpoint;
  cursor?: string;
}

export function createMakoLiveStreamGroup(
  configs: readonly NormalizedMakoRxdbClientConfig[],
  auth: MakoAuthClient,
  options: MakoLiveStreamOptions = {},
): MakoLiveStreamGroup {
  return new MakoLiveStreamGroup(configs, auth, options);
}
