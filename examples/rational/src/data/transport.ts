/**
 * Every request the app makes passes through here. The transport counts what
 * happened on the wire for the diagnostics the tests read, keeps live streams
 * abortable, and can simulate a lost network — a browser test cannot unplug a
 * cable, so "offline" is a transport that fails the way a dead network does.
 */
export interface TransportCounters {
  readonly authRequests: number;
  readonly refreshes: number;
  readonly pullRequests: number;
  readonly pushRequests: number;
  readonly acceptedWrites: number;
  readonly conflictResponses: number;
  readonly deniedWrites: number;
  readonly streamConnections: number;
  readonly failedRequests: number;
}

export class Transport {
  readonly #base: typeof globalThis.fetch;
  readonly #streamAborts = new Set<AbortController>();
  readonly #listeners = new Set<() => void>();
  #online = true;
  #counters = {
    authRequests: 0,
    refreshes: 0,
    pullRequests: 0,
    pushRequests: 0,
    acceptedWrites: 0,
    conflictResponses: 0,
    deniedWrites: 0,
    streamConnections: 0,
    failedRequests: 0,
  };

  constructor(base: typeof globalThis.fetch, online = true) {
    this.#base = base;
    this.#online = online;
  }

  get online(): boolean {
    return this.#online;
  }

  counters(): TransportCounters {
    return { ...this.#counters };
  }

  onChange(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  setOnline(online: boolean): void {
    this.#online = online;
    if (!online) this.disconnectStreams();
    this.#notify();
  }

  disconnectStreams(): void {
    for (const controller of [...this.#streamAborts]) {
      controller.abort();
    }
    this.#streamAborts.clear();
  }

  readonly fetch: typeof globalThis.fetch = async (input, init = {}) => {
    const url = requestUrl(input);
    if (!this.#online) {
      this.#counters.failedRequests += 1;
      this.#notify();
      throw new TypeError("simulated offline network");
    }
    if (url.pathname.includes("/auth/")) {
      this.#counters.authRequests += 1;
      if (url.pathname.endsWith("/auth/token")) this.#counters.refreshes += 1;
    }
    if (url.pathname.endsWith("/replication/pull")) this.#counters.pullRequests += 1;
    if (url.pathname.endsWith("/replication/push")) this.#counters.pushRequests += 1;

    if (url.pathname.endsWith("/replication/stream")) {
      // The stream body outlives the fetch promise; keep it abortable for as
      // long as it is open so going offline can cut it.
      const controller = new AbortController();
      const upstream = init.signal;
      upstream?.addEventListener("abort", () => controller.abort(), { once: true });
      this.#streamAborts.add(controller);
      this.#counters.streamConnections += 1;
      controller.signal.addEventListener("abort", () => this.#streamAborts.delete(controller), {
        once: true,
      });
      this.#notify();
      try {
        return await this.#base(input, { ...init, signal: controller.signal });
      } catch (error) {
        this.#streamAborts.delete(controller);
        this.#counters.failedRequests += 1;
        throw error;
      }
    }

    let response: Response;
    try {
      response = await this.#base(input, init);
    } catch (error) {
      this.#counters.failedRequests += 1;
      this.#notify();
      throw error;
    }
    if (url.pathname.endsWith("/replication/push") && response.ok) {
      const body = (await response.clone().json()) as {
        outcomes?: Array<{ status?: string }>;
      };
      for (const outcome of body.outcomes ?? []) {
        if (outcome.status === "accepted") this.#counters.acceptedWrites += 1;
        if (outcome.status === "conflict") this.#counters.conflictResponses += 1;
        if (outcome.status === "denied") this.#counters.deniedWrites += 1;
      }
    }
    this.#notify();
    return response;
  };

  #notify(): void {
    for (const listener of this.#listeners) listener();
  }
}

export function requestUrl(input: RequestInfo | URL): URL {
  if (typeof input === "string") return new URL(input, window.location.origin);
  if (input instanceof URL) return input;
  return new URL(input.url);
}
