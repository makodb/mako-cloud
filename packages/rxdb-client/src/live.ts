import type { RxReplicationPullStreamItem, WithDeleted } from "rxdb";
import { Subject, type Observable } from "rxjs";

import { MakoAuthenticationRequiredError, type MakoAuthClient } from "./auth.js";
import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import type { MakoCheckpoint } from "./pull.js";
import { replicationNetworkError, replicationResponseError } from "./replication-error.js";

export interface MakoLiveStreamOptions {
  readonly fetch?: typeof globalThis.fetch;
  readonly maximumBufferedEvents?: number;
  readonly reconnectMinimumDelayMs?: number;
  readonly reconnectMaximumDelayMs?: number;
  readonly onResyncReason?: (reason: MakoResyncReason) => Promise<void> | void;
}

export type MakoResyncReason =
  | "reconnected"
  | "stream_gap"
  | "checkpoint_expired"
  | "authorization_epoch_changed"
  | "service_failover";

export class MakoLivePullStream<RxDocType> {
  readonly #config: NormalizedMakoRxdbClientConfig;
  readonly #auth: MakoAuthClient;
  readonly #fetch: typeof globalThis.fetch;
  readonly #maximumBufferedEvents: number;
  readonly #minimumDelay: number;
  readonly #maximumDelay: number;
  readonly #onResyncReason: ((reason: MakoResyncReason) => Promise<void> | void) | undefined;
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
        if (error instanceof MakoAuthenticationRequiredError) {
          this.#subject.error(error);
          this.#closed = true;
          break;
        }
        if (error instanceof Error && error.name === "AbortError") {
          reconnecting = true;
          continue;
        }
        reconnecting = true;
        continue;
      }
      reconnecting = true;
    }
    this.#running = false;
  }

  async #connect(): Promise<void> {
    const accessToken = await this.#auth.validAccessToken();
    const abort = new AbortController();
    this.#abort = abort;
    let response: Response;
    try {
      response = await this.#fetch(streamUrl(this.#config, this.#checkpoint, this.#cursor), {
        headers: {
          Accept: "text/event-stream",
          Authorization: `Bearer ${accessToken}`,
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
    if (!response.ok) {
      throw await replicationResponseError(response);
    }
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
      this.#checkpoint = checkpoint;
      this.#enqueue({
        documents: event.data.documents as WithDeleted<RxDocType>[],
        checkpoint,
      });
    } else if (event.event === "checkpoint") {
      const checkpoint = { token: event.data.checkpoint };
      this.#checkpoint = checkpoint;
      this.#enqueue({ documents: [], checkpoint });
    } else if (event.event === "resync") {
      if (this.#onResyncReason !== undefined) {
        void this.#onResyncReason(event.data.reason);
      }
      this.#enqueue("RESYNC");
    }
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
  if (cursor !== undefined) {
    url.searchParams.set("cursor", cursor);
  } else if (checkpoint !== undefined) {
    url.searchParams.set("checkpoint", checkpoint.token);
  }
  return url;
}

type LiveEvent =
  | {
      event: "documents";
      data: { documents: unknown[]; checkpoint: string; cursor: string };
    }
  | { event: "checkpoint"; data: { checkpoint: string; cursor: string } }
  | { event: "heartbeat"; data: { cursor: string } }
  | { event: "resync"; data: { reason: MakoResyncReason } };

function isLiveEvent(value: unknown): value is LiveEvent {
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

function boundedInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new TypeError(`value must be an integer between ${minimum} and ${maximum}`);
  }
  return value;
}

function wait(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
